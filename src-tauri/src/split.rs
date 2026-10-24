//! Splits a PDF into one-page files or custom page ranges.
//!
//! Supported: selected page order, page dimensions, rotation, vector content,
//! fonts, images, inherited page resources, and direct internal link
//! annotations whose destination page is part of the same output.
//!
//! Not preserved: bookmarks/outlines, named destinations, AcroForm/XFA fields,
//! tagged-PDF structure, optional content groups, page labels, document-level
//! metadata, digital signatures, and direct links to pages outside the output.
//! Encrypted PDFs are rejected and no password flow is provided.
//!
//! Each output is rebuilt with only the objects reachable from its selected
//! pages. Catalog-level structures from the source are intentionally dropped.
//!
//! The same page-copying path also backs Extract Pages and Delete Pages. Those
//! actions write one selected-page PDF rather than a set of split files.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId};
use tempfile::NamedTempFile;

use crate::naming;

pub const MAX_SPLIT_PAGES: usize = 2_000;
pub const MAX_SPLIT_OUTPUTS: usize = 500;
pub const MAX_SPLIT_RANGES: usize = 100;
pub const MAX_SPLIT_INPUT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PAGE_TREE_DEPTH: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitMode {
    EveryPage,
    Ranges,
}

impl SplitMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "every" => Some(Self::EveryPage),
            "ranges" => Some(Self::Ranges),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitOutputPlan {
    pub name: String,
    pub pages: Vec<usize>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPlan {
    pub page_count: usize,
    pub outputs: Vec<SplitOutputPlan>,
    pub excluded_pages: Vec<usize>,
}

impl SplitPlan {
    pub fn included_pages(&self) -> usize {
        self.outputs.iter().map(|output| output.pages.len()).sum()
    }
}

#[derive(Clone, Debug)]
pub struct SplitSource {
    pub pages: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct SplitFileOutcome {
    pub name: String,
    pub pages: usize,
    pub bytes_written: u64,
}

#[derive(Clone, Debug)]
pub struct SplitOutcome {
    pub files: Vec<SplitFileOutcome>,
    pub pages: usize,
    pub bytes_written: u64,
}

#[derive(Debug)]
pub enum SplitError {
    InputNotFound { path: PathBuf },
    InputRead { path: PathBuf, source: io::Error },
    InputTooLarge { path: PathBuf, bytes: u64, max: u64 },
    InvalidPdf { path: PathBuf, reason: String },
    EncryptedPdf { path: PathBuf },
    InvalidReference { path: PathBuf, reference: String },
    NoPages,
    TooManyPages { max: usize },
    TooManyOutputs { max: usize },
    TooManyRanges { max: usize },
    InvalidRanges(String),
    OutputDirectoryInvalid { path: PathBuf },
    OutputMatchesInput { path: PathBuf },
    OutputCollision { paths: Vec<PathBuf> },
    OutputWrite { path: PathBuf, source: io::Error },
}

impl fmt::Display for SplitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputNotFound { path } => write!(formatter, "PDF not found: {}", path.display()),
            Self::InputRead { path, source } => {
                write!(formatter, "Could not read {}: {source}", path.display())
            }
            Self::InputTooLarge { path, bytes, max } => write!(
                formatter,
                "{} is too large to split ({:.0} MB). The limit is {:.0} MB.",
                path.display(),
                *bytes as f64 / (1024.0 * 1024.0),
                *max as f64 / (1024.0 * 1024.0)
            ),
            Self::InvalidPdf { path, reason } => {
                write!(formatter, "Invalid PDF {}: {reason}", path.display())
            }
            Self::EncryptedPdf { path } => write!(
                formatter,
                "Encrypted PDFs are not supported: {}",
                path.display()
            ),
            Self::InvalidReference { path, reference } => write!(
                formatter,
                "Invalid reference in {}: object {reference} is missing.",
                path.display()
            ),
            Self::NoPages => write!(formatter, "This PDF has no pages."),
            Self::TooManyPages { max } => write!(
                formatter,
                "This PDF has too many pages to split. The limit is {max} pages."
            ),
            Self::TooManyOutputs { max } => write!(
                formatter,
                "This split would create too many files. The limit is {max} output files."
            ),
            Self::TooManyRanges { max } => {
                write!(formatter, "Too many page ranges. Use at most {max} ranges.")
            }
            Self::InvalidRanges(message) => formatter.write_str(message),
            Self::OutputDirectoryInvalid { path } => write!(
                formatter,
                "Output folder does not exist: {}",
                path.display()
            ),
            Self::OutputMatchesInput { path } => write!(
                formatter,
                "The output must not overwrite the source PDF: {}",
                path.display()
            ),
            Self::OutputCollision { paths } => {
                let names: Vec<String> = paths
                    .iter()
                    .take(5)
                    .map(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string())
                    })
                    .collect();
                let extra = paths.len().saturating_sub(names.len());
                let suffix = if extra > 0 {
                    format!(" and {extra} more")
                } else {
                    String::new()
                };
                write!(
                    formatter,
                    "These files already exist in the destination: {}{suffix}. Choose another folder or remove them first.",
                    names.join(", ")
                )
            }
            Self::OutputWrite { path, source } => {
                write!(formatter, "Could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for SplitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InputRead { source, .. } | Self::OutputWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
struct InheritedAttributes {
    resources: Option<Object>,
    media_box: Option<Object>,
    crop_box: Option<Object>,
    rotate: Option<Object>,
}

impl InheritedAttributes {
    fn merged_with(&self, dictionary: &Dictionary) -> Self {
        let mut merged = self.clone();
        if let Ok(value) = dictionary.get(b"Resources") {
            merged.resources = Some(value.clone());
        }
        if let Ok(value) = dictionary.get(b"MediaBox") {
            merged.media_box = Some(value.clone());
        }
        if let Ok(value) = dictionary.get(b"CropBox") {
            merged.crop_box = Some(value.clone());
        }
        if let Ok(value) = dictionary.get(b"Rotate") {
            merged.rotate = Some(value.clone());
        }
        merged
    }
}

#[derive(Clone)]
struct PageEntry {
    id: ObjectId,
    dictionary: Dictionary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PageRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub fn inspect_pdf(input: &Path) -> Result<SplitSource, SplitError> {
    let (document, bytes) = load_document(input)?;
    let pages = page_entries(&document, input)?.len();
    validate_page_count(pages)?;

    Ok(SplitSource { pages, bytes })
}

pub fn plan_split(
    page_count: usize,
    file_stem: &str,
    mode: SplitMode,
    ranges: &str,
) -> Result<SplitPlan, SplitError> {
    validate_page_count(page_count)?;
    let stem = if file_stem.trim().is_empty() {
        "Document"
    } else {
        file_stem.trim()
    };

    let ranges = match mode {
        SplitMode::EveryPage => {
            if page_count > MAX_SPLIT_OUTPUTS {
                return Err(SplitError::TooManyOutputs {
                    max: MAX_SPLIT_OUTPUTS,
                });
            }
            (1..=page_count)
                .map(|page| PageRange {
                    start: page,
                    end: page,
                })
                .collect::<Vec<_>>()
        }
        SplitMode::Ranges => parse_ranges(ranges, page_count, true)?,
    };

    if ranges.len() > MAX_SPLIT_OUTPUTS {
        return Err(SplitError::TooManyOutputs {
            max: MAX_SPLIT_OUTPUTS,
        });
    }

    let included: HashSet<usize> = ranges
        .iter()
        .flat_map(|range| range.start..=range.end)
        .collect();
    let excluded_pages = (1..=page_count)
        .filter(|page| !included.contains(page))
        .collect::<Vec<_>>();

    let outputs = ranges
        .into_iter()
        .map(|range| SplitOutputPlan {
            name: naming::split_output_name(stem, range.start, range.end),
            pages: (range.start..=range.end).collect(),
        })
        .collect();

    Ok(SplitPlan {
        page_count,
        outputs,
        excluded_pages,
    })
}

pub fn split_pdf(
    input: &Path,
    output_directory: &Path,
    mode: SplitMode,
    ranges: &str,
) -> Result<SplitOutcome, SplitError> {
    let (document, _) = load_document(input)?;
    let entries = page_entries(&document, input)?;
    validate_page_count(entries.len())?;

    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Document".to_string());
    let plan = plan_split(entries.len(), &stem, mode, ranges)?;

    validate_output_directory(output_directory)?;
    preflight_outputs(output_directory, &plan)?;

    let mut files = Vec::with_capacity(plan.outputs.len());
    let mut created: Vec<PathBuf> = Vec::new();
    let mut total_bytes = 0u64;
    let mut total_pages = 0usize;

    for output in &plan.outputs {
        let target = output_directory.join(&output.name);
        let bytes = match build_output(&document, &entries, &output.pages, input) {
            Ok(bytes) => bytes,
            Err(error) => {
                cleanup_created(&created);
                return Err(error);
            }
        };

        if let Err(error) = write_safely(&target, &bytes, false) {
            cleanup_created(&created);
            return Err(error);
        }

        created.push(target);
        total_bytes += bytes.len() as u64;
        total_pages += output.pages.len();
        files.push(SplitFileOutcome {
            name: output.name.clone(),
            pages: output.pages.len(),
            bytes_written: bytes.len() as u64,
        });
    }

    Ok(SplitOutcome {
        files,
        pages: total_pages,
        bytes_written: total_bytes,
    })
}

#[derive(Clone, Debug)]
pub struct PageSelectionOutcome {
    pub pages: usize,
    pub bytes_written: u64,
}

/// Writes one PDF containing `pages` in the requested order.
///
/// This reuses the split page-copying path: dimensions, rotation, content,
/// fonts, images, resources, and inherited page attributes are preserved.
/// Direct links are retained when both ends are selected; links to excluded
/// pages are dropped.
pub fn write_page_selection(
    input: &Path,
    output: &Path,
    pages: &[usize],
    allow_overwrite: bool,
) -> Result<PageSelectionOutcome, SplitError> {
    let (document, _) = load_document(input)?;
    let entries = page_entries(&document, input)?;
    validate_page_count(entries.len())?;

    if pages.is_empty() {
        return Err(SplitError::InvalidRanges(
            "Select at least one page.".to_string(),
        ));
    }

    let mut seen = HashSet::new();
    for &page in pages {
        if page == 0 || page > entries.len() {
            return Err(SplitError::InvalidRanges(format!(
                "Page {page} is outside this PDF. It has {} {}.",
                entries.len(),
                if entries.len() == 1 { "page" } else { "pages" }
            )));
        }
        if !seen.insert(page) {
            return Err(SplitError::InvalidRanges(format!(
                "Page {page} appears more than once in the selection."
            )));
        }
    }

    validate_single_output(input, output, allow_overwrite)?;

    let bytes = build_output(&document, &entries, pages, input)?;
    write_safely(output, &bytes, allow_overwrite)?;

    Ok(PageSelectionOutcome {
        pages: pages.len(),
        bytes_written: bytes.len() as u64,
    })
}

fn validate_single_output(
    input: &Path,
    output: &Path,
    allow_overwrite: bool,
) -> Result<(), SplitError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    if !parent.is_dir() {
        return Err(SplitError::OutputDirectoryInvalid {
            path: parent.to_path_buf(),
        });
    }

    let parent = fs::canonicalize(parent).map_err(|_| SplitError::OutputDirectoryInvalid {
        path: parent.to_path_buf(),
    })?;
    let file_name = output
        .file_name()
        .ok_or_else(|| SplitError::OutputDirectoryInvalid {
            path: parent.clone(),
        })?;
    let candidate = parent.join(file_name);
    let input_canonical = fs::canonicalize(input).map_err(|_| SplitError::InvalidPdf {
        path: input.to_path_buf(),
        reason: "could not resolve the source path".to_string(),
    })?;
    let output_canonical = if output.exists() {
        fs::canonicalize(output).map_err(|_| SplitError::OutputDirectoryInvalid {
            path: output.to_path_buf(),
        })?
    } else {
        candidate
    };

    if output_canonical == input_canonical {
        return Err(SplitError::OutputMatchesInput {
            path: output.to_path_buf(),
        });
    }

    #[cfg(unix)]
    if output.exists() {
        use std::os::unix::fs::MetadataExt;

        if let (Ok(input_metadata), Ok(output_metadata)) =
            (fs::metadata(input), fs::metadata(output))
            && input_metadata.dev() == output_metadata.dev()
            && input_metadata.ino() == output_metadata.ino()
        {
            return Err(SplitError::OutputMatchesInput {
                path: output.to_path_buf(),
            });
        }
    }

    if output.exists() {
        if output.is_dir() {
            return Err(SplitError::OutputWrite {
                path: output.to_path_buf(),
                source: io::Error::new(io::ErrorKind::AlreadyExists, "output path is a directory"),
            });
        }
        if !allow_overwrite {
            return Err(SplitError::OutputCollision {
                paths: vec![output.to_path_buf()],
            });
        }
    }

    Ok(())
}

fn load_document(path: &Path) -> Result<(Document, u64), SplitError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SplitError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(SplitError::InputRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if metadata.len() > MAX_SPLIT_INPUT_BYTES {
        return Err(SplitError::InputTooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            max: MAX_SPLIT_INPUT_BYTES,
        });
    }
    if metadata.is_dir() {
        return Err(SplitError::InvalidPdf {
            path: path.to_path_buf(),
            reason: "path is a directory".to_string(),
        });
    }

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SplitError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(SplitError::InputRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let document = Document::load_mem(&bytes).map_err(|error| SplitError::InvalidPdf {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;

    if document.is_encrypted() {
        return Err(SplitError::EncryptedPdf {
            path: path.to_path_buf(),
        });
    }

    validate_references(&document, path)?;

    Ok((document, bytes.len() as u64))
}

fn validate_page_count(page_count: usize) -> Result<(), SplitError> {
    if page_count == 0 {
        return Err(SplitError::NoPages);
    }
    if page_count > MAX_SPLIT_PAGES {
        return Err(SplitError::TooManyPages {
            max: MAX_SPLIT_PAGES,
        });
    }
    Ok(())
}

pub(crate) fn parse_page_ranges(
    input: &str,
    page_count: usize,
) -> Result<Vec<PageRange>, SplitError> {
    parse_ranges(input, page_count, false)
}

fn parse_ranges(
    input: &str,
    page_count: usize,
    require_ascending: bool,
) -> Result<Vec<PageRange>, SplitError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SplitError::InvalidRanges(
            "Enter at least one page range, such as 1-3, 5.".to_string(),
        ));
    }

    let tokens: Vec<&str> = trimmed.split(',').map(str::trim).collect();
    if tokens.len() > MAX_SPLIT_RANGES {
        return Err(SplitError::TooManyRanges {
            max: MAX_SPLIT_RANGES,
        });
    }

    let mut ranges: Vec<PageRange> = Vec::with_capacity(tokens.len());
    let mut previous_end = 0usize;

    for (index, token) in tokens.iter().enumerate() {
        if token.is_empty() {
            return Err(SplitError::InvalidRanges(format!(
                "Range {} is empty. Use a format such as 1-3, 5.",
                index + 1
            )));
        }

        let (start, end) = parse_token(token).map_err(|reason| {
            SplitError::InvalidRanges(format!("Invalid range \"{token}\": {reason}"))
        })?;

        if start > end {
            return Err(SplitError::InvalidRanges(format!(
                "Range \"{token}\" is reversed. Start with the lower page number."
            )));
        }
        if end > page_count {
            return Err(SplitError::InvalidRanges(format!(
                "Range \"{token}\" is outside this PDF. It has {page_count} {}.",
                if page_count == 1 { "page" } else { "pages" }
            )));
        }
        if require_ascending && start <= previous_end {
            return Err(SplitError::InvalidRanges(format!(
                "Ranges must be ascending and must not overlap. \"{token}\" overlaps an earlier range."
            )));
        }
        if ranges
            .iter()
            .any(|range| start <= range.end && end >= range.start)
        {
            return Err(SplitError::InvalidRanges(format!(
                "Ranges must not overlap or repeat pages. \"{token}\" overlaps an earlier range."
            )));
        }

        ranges.push(PageRange { start, end });
        previous_end = end;
    }

    Ok(ranges)
}

fn parse_token(token: &str) -> Result<(usize, usize), &'static str> {
    let mut parts = token.split('-');
    let start = parts.next().unwrap_or("").trim();
    let end = parts.next();

    if parts.next().is_some() {
        return Err("use one hyphen, such as 1-3");
    }

    let start = parse_page_number(start)?;
    let end = match end {
        Some(end) => parse_page_number(end.trim())?,
        None => start,
    };

    Ok((start, end))
}

fn parse_page_number(value: &str) -> Result<usize, &'static str> {
    if value.is_empty() {
        return Err("a page number is missing");
    }
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("page numbers must be digits");
    }

    let number = value
        .parse::<usize>()
        .map_err(|_| "page number is too large")?;
    if number == 0 {
        return Err("page numbers start at 1");
    }

    Ok(number)
}

fn page_entries(document: &Document, source: &Path) -> Result<Vec<PageEntry>, SplitError> {
    let root_id = document
        .catalog()
        .map_err(|error| SplitError::InvalidPdf {
            path: source.to_path_buf(),
            reason: error.to_string(),
        })?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| SplitError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "catalog has no page tree".to_string(),
        })?;

    let mut pages = Vec::new();
    let mut visited = HashSet::new();
    collect_pages(
        document,
        root_id,
        &InheritedAttributes::default(),
        &mut visited,
        &mut pages,
        0,
        source,
    )?;

    if pages.is_empty() {
        return Err(SplitError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "document has no pages".to_string(),
        });
    }

    Ok(pages)
}

fn collect_pages(
    document: &Document,
    node_id: ObjectId,
    inherited: &InheritedAttributes,
    visited: &mut HashSet<ObjectId>,
    pages: &mut Vec<PageEntry>,
    depth: usize,
    source: &Path,
) -> Result<(), SplitError> {
    if depth > MAX_PAGE_TREE_DEPTH {
        return Err(SplitError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "page tree is too deep or cyclic".to_string(),
        });
    }
    if !visited.insert(node_id) {
        return Err(SplitError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "page tree contains a cycle".to_string(),
        });
    }

    let node = document
        .get_object(node_id)
        .and_then(Object::as_dict)
        .map_err(|error| SplitError::InvalidPdf {
            path: source.to_path_buf(),
            reason: error.to_string(),
        })?;

    let current = inherited.merged_with(node);

    if let Ok(kids) = node.get(b"Kids").and_then(Object::as_array) {
        for kid in kids {
            let kid_id = kid.as_reference().map_err(|_| SplitError::InvalidPdf {
                path: source.to_path_buf(),
                reason: "page tree kid is not an indirect reference".to_string(),
            })?;
            collect_pages(
                document,
                kid_id,
                &current,
                visited,
                pages,
                depth + 1,
                source,
            )?;
        }
    } else {
        let mut dictionary = node.clone();
        for (key, value) in [
            (b"Resources".as_slice(), current.resources.as_ref()),
            (b"MediaBox".as_slice(), current.media_box.as_ref()),
            (b"CropBox".as_slice(), current.crop_box.as_ref()),
            (b"Rotate".as_slice(), current.rotate.as_ref()),
        ] {
            if dictionary.get(key).is_err()
                && let Some(value) = value
            {
                dictionary.set(key, value.clone());
            }
        }
        pages.push(PageEntry {
            id: node_id,
            dictionary,
        });
    }

    Ok(())
}

fn validate_references(document: &Document, source: &Path) -> Result<(), SplitError> {
    let mut visited = HashSet::new();
    let mut stack: Vec<Object> = Vec::new();

    if let Ok(root) = document.trailer.get(b"Root") {
        stack.push(root.clone());
    }
    if let Ok(info) = document.trailer.get(b"Info") {
        stack.push(info.clone());
    }

    while let Some(object) = stack.pop() {
        match object {
            Object::Reference(id) => {
                let target = document
                    .get_object(id)
                    .map_err(|_| SplitError::InvalidReference {
                        path: source.to_path_buf(),
                        reference: format!("{} {}", id.0, id.1),
                    })?;
                if visited.insert(id) {
                    stack.push(target.clone());
                }
            }
            Object::Array(items) => stack.extend(items),
            Object::Dictionary(dictionary) => {
                stack.extend(dictionary.iter().map(|(_, value)| value.clone()));
            }
            Object::Stream(stream) => {
                stack.extend(stream.dict.iter().map(|(_, value)| value.clone()));
            }
            _ => {}
        }
    }

    Ok(())
}

fn validate_output_directory(directory: &Path) -> Result<(), SplitError> {
    if !directory.is_dir() {
        return Err(SplitError::OutputDirectoryInvalid {
            path: directory.to_path_buf(),
        });
    }
    Ok(())
}

fn preflight_outputs(directory: &Path, plan: &SplitPlan) -> Result<(), SplitError> {
    let mut collisions = Vec::new();
    let mut seen = HashSet::new();

    for output in &plan.outputs {
        if !seen.insert(output.name.as_str()) {
            return Err(SplitError::InvalidRanges(format!(
                "The ranges produce the same output name: {}",
                output.name
            )));
        }

        let target = directory.join(&output.name);
        if target.exists() {
            collisions.push(target);
        }
    }

    if collisions.is_empty() {
        Ok(())
    } else {
        Err(SplitError::OutputCollision { paths: collisions })
    }
}

fn build_output(
    document: &Document,
    entries: &[PageEntry],
    page_numbers: &[usize],
    source: &Path,
) -> Result<Vec<u8>, SplitError> {
    if page_numbers.is_empty() {
        return Err(SplitError::InvalidRanges(
            "A split output must contain at least one page.".to_string(),
        ));
    }

    let selected_ids: HashSet<ObjectId> = page_numbers
        .iter()
        .filter_map(|number| entries.get(number.saturating_sub(1)))
        .map(|entry| entry.id)
        .collect();

    if selected_ids.len() != page_numbers.len() {
        return Err(SplitError::InvalidRanges(
            "The page ranges do not match this PDF.".to_string(),
        ));
    }

    let mut output = Document::with_version(document.version.as_str());
    output.max_id = document.max_id;
    let mut page_ids = Vec::with_capacity(page_numbers.len());

    for page_number in page_numbers {
        let entry = &entries[page_number - 1];
        let mut dictionary = entry.dictionary.clone();
        dictionary.remove(b"Parent");

        let annots = dictionary
            .get(b"Annots")
            .ok()
            .and_then(|value| resolve_object(document, value))
            .and_then(|object| object.as_array().ok())
            .cloned();

        if let Some(annots) = annots {
            let kept = annots
                .iter()
                .filter(|annotation| keep_annotation(document, annotation, &selected_ids))
                .cloned()
                .collect::<Vec<_>>();
            if kept.is_empty() {
                dictionary.remove(b"Annots");
            } else {
                dictionary.set("Annots", Object::Array(kept));
            }
        }

        output
            .objects
            .insert(entry.id, Object::Dictionary(dictionary));
        page_ids.push(entry.id);
    }

    let new_pages_id = output.new_object_id();
    let new_catalog_id = output.new_object_id();
    let mut visited = HashSet::new();

    for page_id in &page_ids {
        if let Some(page) = output.objects.get(page_id).cloned() {
            copy_references(
                document,
                &mut output,
                &page,
                &selected_ids,
                &mut visited,
                source,
            )?;
        }
    }

    for page_id in &page_ids {
        if let Ok(page) = output
            .get_object_mut(*page_id)
            .and_then(Object::as_dict_mut)
        {
            page.set("Parent", new_pages_id);
        }
    }

    let mut pages_dictionary = Dictionary::new();
    pages_dictionary.set("Type", "Pages");
    pages_dictionary.set(
        "Kids",
        page_ids
            .iter()
            .map(|id| Object::Reference(*id))
            .collect::<Vec<_>>(),
    );
    pages_dictionary.set("Count", page_ids.len() as i32);
    output
        .objects
        .insert(new_pages_id, Object::Dictionary(pages_dictionary));

    let mut catalog = Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", new_pages_id);
    output
        .objects
        .insert(new_catalog_id, Object::Dictionary(catalog));
    output.trailer.set("Root", new_catalog_id);

    validate_references(&output, source)?;

    let mut bytes = Vec::new();
    output
        .save_to(&mut bytes)
        .map_err(|error| SplitError::InvalidPdf {
            path: source.to_path_buf(),
            reason: error.to_string(),
        })?;

    Ok(bytes)
}

fn copy_references(
    source: &Document,
    output: &mut Document,
    object: &Object,
    selected_pages: &HashSet<ObjectId>,
    visited: &mut HashSet<ObjectId>,
    source_path: &Path,
) -> Result<(), SplitError> {
    match object {
        Object::Reference(id) => {
            if selected_pages.contains(id) || output.objects.contains_key(id) {
                return Ok(());
            }
            if !visited.insert(*id) {
                return Ok(());
            }

            let target = source
                .get_object(*id)
                .map_err(|_| SplitError::InvalidReference {
                    path: source_path.to_path_buf(),
                    reference: format!("{} {}", id.0, id.1),
                })?;
            let type_name = target.type_name().unwrap_or(b"");
            if type_name == b"Page" || type_name == b"Pages" || type_name == b"Catalog" {
                return Ok(());
            }

            output.objects.insert(*id, target.clone());
            copy_references(source, output, target, selected_pages, visited, source_path)
        }
        Object::Array(items) => {
            for item in items {
                copy_references(source, output, item, selected_pages, visited, source_path)?;
            }
            Ok(())
        }
        Object::Dictionary(dictionary) => {
            for (key, value) in dictionary.iter() {
                if key == b"Parent" {
                    continue;
                }
                copy_references(source, output, value, selected_pages, visited, source_path)?;
            }
            Ok(())
        }
        Object::Stream(stream) => {
            for (key, value) in stream.dict.iter() {
                if key == b"Parent" {
                    continue;
                }
                copy_references(source, output, value, selected_pages, visited, source_path)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn keep_annotation(
    document: &Document,
    annotation: &Object,
    selected_pages: &HashSet<ObjectId>,
) -> bool {
    let Some(dictionary) =
        resolve_object(document, annotation).and_then(|object| object.as_dict().ok())
    else {
        return true;
    };

    if let Ok(parent_page) = dictionary.get(b"P").and_then(Object::as_reference)
        && !selected_pages.contains(&parent_page)
    {
        return false;
    }

    match annotation_destination(document, dictionary) {
        Some(destination) => {
            destination_page_is_outside(document, destination, selected_pages) != Some(true)
        }
        None => true,
    }
}

fn annotation_destination<'a>(
    document: &'a Document,
    dictionary: &'a Dictionary,
) -> Option<&'a Object> {
    if let Ok(destination) = dictionary.get(b"Dest") {
        return Some(destination);
    }

    let action = dictionary.get(b"A").ok()?;
    let action = resolve_object(document, action)?.as_dict().ok()?;
    let is_goto = action
        .get(b"S")
        .and_then(Object::as_name)
        .is_ok_and(|name| name == b"GoTo");
    if !is_goto {
        return None;
    }

    action.get(b"D").ok()
}

fn destination_page_is_outside(
    document: &Document,
    destination: &Object,
    selected_pages: &HashSet<ObjectId>,
) -> Option<bool> {
    let destination = resolve_object(document, destination)?;
    match destination {
        Object::Name(_) | Object::String(_, _) => Some(true),
        Object::Array(items) => items
            .first()
            .and_then(|item| item.as_reference().ok())
            .map(|id| !selected_pages.contains(&id)),
        Object::Dictionary(dictionary) => dictionary
            .get(b"D")
            .ok()
            .and_then(|value| destination_page_is_outside(document, value, selected_pages)),
        _ => None,
    }
}

fn resolve_object<'a>(document: &'a Document, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => document.get_object(*id).ok(),
        _ => Some(object),
    }
}

fn write_safely(target: &Path, bytes: &[u8], allow_overwrite: bool) -> Result<(), SplitError> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temp = NamedTempFile::new_in(parent).map_err(|source| SplitError::OutputWrite {
        path: target.to_path_buf(),
        source,
    })?;

    temp.write_all(bytes)
        .map_err(|source| SplitError::OutputWrite {
            path: target.to_path_buf(),
            source,
        })?;
    temp.as_file()
        .sync_all()
        .map_err(|source| SplitError::OutputWrite {
            path: target.to_path_buf(),
            source,
        })?;

    let persisted = if allow_overwrite {
        temp.persist(target)
    } else {
        temp.persist_noclobber(target)
    };

    match persisted {
        Ok(file) => {
            drop(file);
            if let Ok(directory) = fs::File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        }
        Err(error) => {
            if !allow_overwrite && error.error.kind() == io::ErrorKind::AlreadyExists {
                Err(SplitError::OutputCollision {
                    paths: vec![target.to_path_buf()],
                })
            } else {
                Err(SplitError::OutputWrite {
                    path: target.to_path_buf(),
                    source: error.error,
                })
            }
        }
    }
}

fn cleanup_created(created: &[PathBuf]) {
    for path in created {
        if let Err(error) = fs::remove_file(path) {
            eprintln!(
                "quicklet: failed to remove partial split output {}: {error}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;
    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::{EncryptionState, EncryptionVersion, Permissions, Stream};
    use pdf_writer::types::{ActionType, AnnotationType};
    use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect as PdfRect, Ref, Str};
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    struct TestPage {
        width: f32,
        height: f32,
        rotate: i32,
        text: String,
        link_to: Option<usize>,
    }

    impl TestPage {
        fn new(
            width: f32,
            height: f32,
            rotate: i32,
            text: impl Into<String>,
            link_to: Option<usize>,
        ) -> Self {
            Self {
                width,
                height,
                rotate,
                text: text.into(),
                link_to,
            }
        }
    }

    fn write_test_pdf(path: &Path, pages: &[TestPage]) {
        let mut pdf = Pdf::new();
        let count = pages.len() as i32;
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let page_refs: Vec<Ref> = (0..count).map(|i| Ref::new(3 + i * 2)).collect();
        let content_refs: Vec<Ref> = (0..count).map(|i| Ref::new(4 + i * 2)).collect();
        let mut next_id = 3 + count * 2;
        let font = Ref::new(next_id);
        next_id += 1;

        let mut annotation_refs: Vec<Option<Ref>> = vec![None; pages.len()];
        for (index, page) in pages.iter().enumerate() {
            if page.link_to.is_some() {
                annotation_refs[index] = Some(Ref::new(next_id));
                next_id += 1;
            }
        }

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree).kids(page_refs.iter().copied()).count(count);

        for (index, page) in pages.iter().enumerate() {
            let mut writer = pdf.page(page_refs[index]);
            writer.media_box(PdfRect::new(0.0, 0.0, page.width, page.height));
            writer.parent(tree);
            writer.contents(content_refs[index]);
            if let Some(annotation) = annotation_refs[index] {
                writer.annotations([annotation]);
            }
            if page.rotate != 0 {
                writer.rotate(page.rotate);
            }
            writer.resources().fonts().pair(Name(b"F1"), font);
        }

        pdf.type1_font(font).base_font(Name(b"Helvetica"));

        for (index, page) in pages.iter().enumerate() {
            let mut content = Content::new();
            content.begin_text();
            content.set_font(Name(b"F1"), 20.0);
            content.next_line(72.0, 700.0);
            content.show(Str(page.text.as_bytes()));
            content.end_text();
            pdf.stream(content_refs[index], &content.finish());
        }

        for (index, page) in pages.iter().enumerate() {
            if let (Some(annotation), Some(target)) = (annotation_refs[index], page.link_to) {
                let mut annotation = pdf.annotation(annotation);
                annotation.subtype(AnnotationType::Link);
                annotation.rect(PdfRect::new(72.0, 650.0, 280.0, 680.0));
                annotation.border_style().width(1.0);
                annotation
                    .action()
                    .action_type(ActionType::GoTo)
                    .destination()
                    .page(page_refs[target])
                    .fit();
                annotation.finish();
            }
        }

        fs::write(path, pdf.finish()).unwrap();
    }

    fn write_image_pdf(path: &Path) {
        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let page = Ref::new(3);
        let font = Ref::new(4);
        let content_id = Ref::new(5);
        let image = Ref::new(6);

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree).kids([page]).count(1);

        let mut pixels = Vec::with_capacity(8 * 8 * 3);
        for y in 0..8 {
            for x in 0..8 {
                pixels.push((x * 32) as u8);
                pixels.push((y * 32) as u8);
                pixels.push(160);
            }
        }
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&pixels, 6);

        {
            let mut writer = pdf.page(page);
            writer.media_box(PdfRect::new(0.0, 0.0, 300.0, 200.0));
            writer.parent(tree);
            writer.contents(content_id);
            let mut resources = writer.resources();
            resources.fonts().pair(Name(b"F1"), font);
            resources.x_objects().pair(Name(b"Im1"), image);
        }

        pdf.type1_font(font).base_font(Name(b"Helvetica"));

        let mut image_writer = pdf.image_xobject(image, &compressed);
        image_writer.filter(Filter::FlateDecode);
        image_writer.width(8);
        image_writer.height(8);
        image_writer.color_space().device_rgb();
        image_writer.bits_per_component(8);
        image_writer.finish();

        let mut content = Content::new();
        content.begin_text();
        content.set_font(Name(b"F1"), 16.0);
        content.next_line(40.0, 160.0);
        content.show(Str(b"C1 image page"));
        content.end_text();
        content.save_state();
        content.transform([80.0, 0.0, 0.0, 80.0, 120.0, 40.0]);
        content.x_object(Name(b"Im1"));
        content.restore_state();
        pdf.stream(content_id, &content.finish());

        fs::write(path, pdf.finish()).unwrap();
    }

    fn write_nested_inherited_pdf(path: &Path) {
        let mut document = Document::with_version("1.5");
        let root = document.new_object_id();
        let middle = document.new_object_id();
        let page = document.new_object_id();

        let font = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources = document.add_object(dictionary! {
            "Font" => dictionary! {
                "F1" => font,
            },
        });
        let content = document.add_object(Stream::new(
            dictionary! {},
            b"BT /F1 20 Tf 72 700 Td (Nested inherited page) Tj ET".to_vec(),
        ));

        document.objects.insert(
            page,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => middle,
                "Contents" => content,
            }),
        );
        document.objects.insert(
            middle,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Parent" => root,
                "Kids" => vec![Object::Reference(page)],
                "Count" => 1,
                "Resources" => Object::Reference(resources),
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        document.objects.insert(
            root,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(middle)],
                "Count" => 1,
            }),
        );

        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => root,
        });
        document.trailer.set("Root", catalog);
        document.save(path).unwrap();
    }

    fn write_encrypted_pdf(source: &Path, output: &Path) {
        let mut document = Document::load(source).unwrap();
        let file_id = Object::string_literal(b"quicklet-split-test");
        document.trailer.set("ID", vec![file_id.clone(), file_id]);

        let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
        let version = EncryptionVersion::V4 {
            document: &document,
            encrypt_metadata: true,
            crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]),
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: "owner",
            user_password: "user",
            permissions: Permissions::PRINTABLE,
        };
        let state = EncryptionState::try_from(version).unwrap();
        document.encrypt(&state).unwrap();
        document.save(output).unwrap();
    }

    fn page_ids(document: &Document) -> Vec<ObjectId> {
        document.get_pages().into_values().collect()
    }

    fn page_dictionary(document: &Document, page_id: ObjectId) -> &Dictionary {
        document.get_object(page_id).unwrap().as_dict().unwrap()
    }

    fn media_box(document: &Document, page_id: ObjectId) -> (f32, f32) {
        let page = page_dictionary(document, page_id);
        let media = page.get(b"MediaBox").unwrap().as_array().unwrap();
        (media[2].as_float().unwrap(), media[3].as_float().unwrap())
    }

    fn rotation(document: &Document, page_id: ObjectId) -> i64 {
        page_dictionary(document, page_id)
            .get(b"Rotate")
            .and_then(Object::as_i64)
            .unwrap_or(0)
    }

    fn page_text(document: &Document, page_id: ObjectId) -> String {
        String::from_utf8_lossy(&document.get_page_content(page_id)).into_owned()
    }

    fn link_targets(document: &Document, page_id: ObjectId) -> Vec<u32> {
        let page_numbers: HashMap<ObjectId, u32> = document
            .get_pages()
            .iter()
            .map(|(number, id)| (*id, *number))
            .collect();
        let mut targets = Vec::new();
        let Ok(annots) = page_dictionary(document, page_id)
            .get(b"Annots")
            .and_then(Object::as_array)
        else {
            return targets;
        };

        for annotation in annots {
            let dictionary = if let Ok(reference) = annotation.as_reference() {
                document
                    .get_object(reference)
                    .and_then(Object::as_dict)
                    .ok()
            } else {
                annotation.as_dict().ok()
            };
            let Some(dictionary) = dictionary else {
                continue;
            };
            let destination = dictionary
                .get(b"A")
                .ok()
                .and_then(|action| action.as_dict().ok())
                .and_then(|action| action.get(b"D").ok())
                .or_else(|| dictionary.get(b"Dest").ok());
            let Some(destination) = destination else {
                continue;
            };
            let Ok(array) = destination.as_array() else {
                continue;
            };
            let Some(first) = array.first() else { continue };
            let Ok(target) = first.as_reference() else {
                continue;
            };
            if let Some(number) = page_numbers.get(&target) {
                targets.push(*number);
            }
        }

        targets
    }

    fn image_count(document: &Document) -> usize {
        document
            .objects
            .values()
            .filter(|object| {
                object
                    .as_stream()
                    .ok()
                    .and_then(|stream| stream.dict.get(b"Subtype").and_then(Object::as_name).ok())
                    .is_some_and(|name| name == b"Image")
            })
            .count()
    }

    fn source_bytes(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()
    }

    #[test]
    fn splits_every_page_and_preserves_page_properties() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Report.pdf");
        let output = dir.path().join("out");
        fs::create_dir(&output).unwrap();
        write_test_pdf(
            &source,
            &[
                TestPage::new(595.0, 842.0, 0, "A1 portrait", None),
                TestPage::new(612.0, 792.0, 90, "B2 rotated", None),
                TestPage::new(400.0, 300.0, 0, "C3 landscape", None),
            ],
        );
        let original = source_bytes(&source);

        let outcome = split_pdf(&source, &output, SplitMode::EveryPage, "").unwrap();

        assert_eq!(outcome.pages, 3);
        assert_eq!(outcome.files.len(), 3);
        assert_eq!(
            outcome
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            [
                "Quicklet-Split-Report-page-001.pdf",
                "Quicklet-Split-Report-page-002.pdf",
                "Quicklet-Split-Report-page-003.pdf"
            ]
        );

        for (index, expected) in ["A1 portrait", "B2 rotated", "C3 landscape"]
            .iter()
            .enumerate()
        {
            let path = output.join(&outcome.files[index].name);
            let document = Document::load(&path).unwrap();
            let ids = page_ids(&document);
            assert_eq!(ids.len(), 1);
            assert!(page_text(&document, ids[0]).contains(expected));
        }

        let first = Document::load(output.join("Quicklet-Split-Report-page-001.pdf")).unwrap();
        assert_eq!(media_box(&first, page_ids(&first)[0]), (595.0, 842.0));

        let second = Document::load(output.join("Quicklet-Split-Report-page-002.pdf")).unwrap();
        assert_eq!(media_box(&second, page_ids(&second)[0]), (612.0, 792.0));
        assert_eq!(rotation(&second, page_ids(&second)[0]), 90);

        let third = Document::load(output.join("Quicklet-Split-Report-page-003.pdf")).unwrap();
        assert_eq!(media_box(&third, page_ids(&third)[0]), (400.0, 300.0));

        assert_eq!(source_bytes(&source), original);
    }

    #[test]
    fn splits_custom_ranges_and_reports_excluded_pages() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Report.pdf");
        let output = dir.path().join("out");
        fs::create_dir(&output).unwrap();
        let pages = (1..=8)
            .map(|number| TestPage::new(500.0, 700.0, 0, format!("Page {number}"), None))
            .collect::<Vec<_>>();
        write_test_pdf(&source, &pages);

        let plan = plan_split(8, "Report", SplitMode::Ranges, "1, 3-5, 8").unwrap();
        assert_eq!(plan.included_pages(), 5);
        assert_eq!(plan.excluded_pages, vec![2, 6, 7]);
        assert_eq!(
            plan.outputs
                .iter()
                .map(|output| output.name.as_str())
                .collect::<Vec<_>>(),
            [
                "Quicklet-Split-Report-page-001.pdf",
                "Quicklet-Split-Report-pages-003-005.pdf",
                "Quicklet-Split-Report-page-008.pdf"
            ]
        );

        let outcome = split_pdf(&source, &output, SplitMode::Ranges, "1, 3-5, 8").unwrap();
        assert_eq!(outcome.pages, 5);
        assert_eq!(outcome.files.len(), 3);
        assert_eq!(outcome.files[1].pages, 3);

        let second =
            Document::load(output.join("Quicklet-Split-Report-pages-003-005.pdf")).unwrap();
        let texts = page_ids(&second)
            .into_iter()
            .map(|id| page_text(&second, id))
            .collect::<Vec<_>>();
        assert!(texts[0].contains("Page 3"));
        assert!(texts[1].contains("Page 4"));
        assert!(texts[2].contains("Page 5"));
    }

    #[test]
    fn rejects_invalid_ranges_with_actionable_messages() {
        let cases = [
            ("", "Enter at least one page range"),
            ("0", "page numbers start at 1"),
            ("1-0", "page numbers start at 1"),
            ("3-1", "reversed"),
            ("2-3, 1-2", "ascending"),
            ("1-2, 2-3", "overlap"),
            ("1,,2", "empty"),
            ("a", "digits"),
            ("1-", "missing"),
            ("-2", "missing"),
            ("1-2-3", "one hyphen"),
            ("9-10", "outside this PDF"),
        ];

        for (input, expected) in cases {
            let error = plan_split(8, "Report", SplitMode::Ranges, input).unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "expected \"{expected}\" in \"{message}\" for input {input:?}"
            );
        }

        let too_many = (1..=MAX_SPLIT_RANGES + 1)
            .map(|number| number.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let error =
            plan_split(MAX_SPLIT_RANGES + 1, "Report", SplitMode::Ranges, &too_many).unwrap_err();
        assert!(matches!(error, SplitError::TooManyRanges { .. }));
    }

    #[test]
    fn preserves_internal_links_and_drops_links_to_other_outputs() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Links.pdf");
        let every = dir.path().join("every");
        let first_two = dir.path().join("first-two");
        let last_two = dir.path().join("last-two");
        fs::create_dir(&every).unwrap();
        fs::create_dir(&first_two).unwrap();
        fs::create_dir(&last_two).unwrap();
        write_test_pdf(
            &source,
            &[
                TestPage::new(500.0, 700.0, 0, "Page 1", Some(1)),
                TestPage::new(500.0, 700.0, 0, "Page 2", Some(0)),
                TestPage::new(500.0, 700.0, 0, "Page 3", Some(0)),
            ],
        );

        split_pdf(&source, &every, SplitMode::EveryPage, "").unwrap();
        let first = Document::load(every.join("Quicklet-Split-Links-page-001.pdf")).unwrap();
        assert!(link_targets(&first, page_ids(&first)[0]).is_empty());

        let first_two_outcome = split_pdf(&source, &first_two, SplitMode::Ranges, "1-2").unwrap();
        assert_eq!(first_two_outcome.files.len(), 1);
        let first_two_doc =
            Document::load(first_two.join("Quicklet-Split-Links-pages-001-002.pdf")).unwrap();
        let ids = page_ids(&first_two_doc);
        assert_eq!(ids.len(), 2);
        assert_eq!(link_targets(&first_two_doc, ids[0]), vec![2]);
        assert_eq!(link_targets(&first_two_doc, ids[1]), vec![1]);

        split_pdf(&source, &last_two, SplitMode::Ranges, "2-3").unwrap();
        let last_two_doc =
            Document::load(last_two.join("Quicklet-Split-Links-pages-002-003.pdf")).unwrap();
        let ids = page_ids(&last_two_doc);
        assert!(link_targets(&last_two_doc, ids[0]).is_empty());
        assert!(link_targets(&last_two_doc, ids[1]).is_empty());
    }

    #[test]
    fn preserves_fonts_images_and_inherited_resources() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Image.pdf");
        let output = dir.path().join("out");
        fs::create_dir(&output).unwrap();
        write_image_pdf(&source);

        split_pdf(&source, &output, SplitMode::EveryPage, "").unwrap();
        let document = Document::load(output.join("Quicklet-Split-Image-page-001.pdf")).unwrap();
        let id = page_ids(&document)[0];
        assert!(page_text(&document, id).contains("C1 image page"));
        assert_eq!(image_count(&document), 1);
        assert!(page_dictionary(&document, id).get(b"Resources").is_ok());
        assert!(page_dictionary(&document, id).get(b"MediaBox").is_ok());

        let nested = dir.path().join("Nested.pdf");
        let nested_output = dir.path().join("nested-out");
        fs::create_dir(&nested_output).unwrap();
        write_nested_inherited_pdf(&nested);
        split_pdf(&nested, &nested_output, SplitMode::EveryPage, "").unwrap();
        let document =
            Document::load(nested_output.join("Quicklet-Split-Nested-page-001.pdf")).unwrap();
        let id = page_ids(&document)[0];
        assert!(page_text(&document, id).contains("Nested inherited page"));
        assert_eq!(media_box(&document, id), (595.0, 842.0));
        assert!(page_dictionary(&document, id).get(b"Resources").is_ok());
    }

    #[test]
    fn rejects_encrypted_malformed_and_invalid_reference_pdfs() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Source.pdf");
        write_test_pdf(&source, &[TestPage::new(500.0, 700.0, 0, "Page", None)]);

        let encrypted = dir.path().join("Encrypted.pdf");
        write_encrypted_pdf(&source, &encrypted);
        let output = dir.path().join("encrypted-out");
        fs::create_dir(&output).unwrap();
        assert!(matches!(
            split_pdf(&encrypted, &output, SplitMode::EveryPage, ""),
            Err(SplitError::EncryptedPdf { .. })
        ));

        let truncated = dir.path().join("Truncated.pdf");
        let bytes = source_bytes(&source);
        fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        let truncated_output = dir.path().join("truncated-out");
        fs::create_dir(&truncated_output).unwrap();
        assert!(matches!(
            split_pdf(&truncated, &truncated_output, SplitMode::EveryPage, ""),
            Err(SplitError::InvalidPdf { .. })
        ));

        let broken = dir.path().join("Broken.pdf");
        let mut document = Document::load(&source).unwrap();
        let page_id = page_ids(&document)[0];
        let page = document
            .get_object_mut(page_id)
            .unwrap()
            .as_dict_mut()
            .unwrap();
        page.set("Contents", Object::Reference((9999, 0)));
        document.save(&broken).unwrap();
        let broken_output = dir.path().join("broken-out");
        fs::create_dir(&broken_output).unwrap();
        assert!(matches!(
            split_pdf(&broken, &broken_output, SplitMode::EveryPage, ""),
            Err(SplitError::InvalidReference { .. })
        ));
    }

    #[test]
    fn rejects_collisions_and_preserves_existing_files() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Report.pdf");
        let output = dir.path().join("out");
        fs::create_dir(&output).unwrap();
        write_test_pdf(
            &source,
            &[
                TestPage::new(500.0, 700.0, 0, "Page 1", None),
                TestPage::new(500.0, 700.0, 0, "Page 2", None),
                TestPage::new(500.0, 700.0, 0, "Page 3", None),
            ],
        );
        let original = source_bytes(&source);
        let existing = output.join("Quicklet-Split-Report-page-002.pdf");
        fs::write(&existing, b"keep me").unwrap();

        let error = split_pdf(&source, &output, SplitMode::EveryPage, "").unwrap_err();
        let SplitError::OutputCollision { paths } = error else {
            panic!("expected output collision");
        };
        assert_eq!(paths, vec![existing.clone()]);
        assert_eq!(fs::read(&existing).unwrap(), b"keep me");
        assert!(!output.join("Quicklet-Split-Report-page-001.pdf").exists());
        assert!(!output.join("Quicklet-Split-Report-page-003.pdf").exists());
        assert_eq!(source_bytes(&source), original);
    }

    #[test]
    fn cleans_up_new_outputs_when_a_later_output_fails() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Report.pdf");
        let output = dir.path().join("out");
        fs::create_dir(&output).unwrap();
        write_test_pdf(
            &source,
            &[
                TestPage::new(500.0, 700.0, 0, "Page 1", None),
                TestPage::new(500.0, 700.0, 0, "Page 2", None),
                TestPage::new(500.0, 700.0, 0, "Page 3", None),
            ],
        );

        let mut document = Document::load(&source).unwrap();
        let ids = page_ids(&document);
        let content_id = page_dictionary(&document, ids[1])
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        document
            .get_object_mut(content_id)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .dict
            .set("UnselectedPage", Object::Reference(ids[2]));
        document.save(&source).unwrap();
        let original = source_bytes(&source);

        let error = split_pdf(&source, &output, SplitMode::EveryPage, "").unwrap_err();
        assert!(matches!(error, SplitError::InvalidReference { .. }));
        assert!(!output.join("Quicklet-Split-Report-page-001.pdf").exists());
        assert!(!output.join("Quicklet-Split-Report-page-002.pdf").exists());
        assert!(!output.join("Quicklet-Split-Report-page-003.pdf").exists());
        assert_eq!(source_bytes(&source), original);
    }

    #[test]
    fn enforces_page_output_and_input_limits() {
        let every_limit =
            plan_split(MAX_SPLIT_OUTPUTS + 1, "Report", SplitMode::EveryPage, "").unwrap_err();
        assert!(matches!(every_limit, SplitError::TooManyOutputs { .. }));

        let page_limit =
            plan_split(MAX_SPLIT_PAGES + 1, "Report", SplitMode::EveryPage, "").unwrap_err();
        assert!(matches!(page_limit, SplitError::TooManyPages { .. }));

        let dir = tempdir().unwrap();
        let huge = dir.path().join("Huge.pdf");
        let file = fs::File::create(&huge).unwrap();
        file.set_len(MAX_SPLIT_INPUT_BYTES + 1).unwrap();
        let error = inspect_pdf(&huge).unwrap_err();
        assert!(matches!(error, SplitError::InputTooLarge { .. }));
    }

    #[test]
    fn supports_repeated_splits_and_invalid_output_directories() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Report.pdf");
        write_test_pdf(
            &source,
            &[
                TestPage::new(500.0, 700.0, 0, "Page 1", None),
                TestPage::new(500.0, 700.0, 0, "Page 2", None),
            ],
        );

        for run in 0..2 {
            let output = dir.path().join(format!("out-{run}"));
            fs::create_dir(&output).unwrap();
            let outcome = split_pdf(&source, &output, SplitMode::Ranges, "1-2").unwrap();
            assert_eq!(outcome.pages, 2);
            assert_eq!(outcome.files.len(), 1);
        }

        let missing = dir.path().join("missing");
        assert!(matches!(
            split_pdf(&source, &missing, SplitMode::EveryPage, ""),
            Err(SplitError::OutputDirectoryInvalid { .. })
        ));
    }

    #[test]
    fn page_selection_writer_validates_pages_and_protects_outputs() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Report.pdf");
        write_test_pdf(
            &source,
            &[
                TestPage::new(500.0, 700.0, 0, "Page 1", None),
                TestPage::new(500.0, 700.0, 0, "Page 2", None),
                TestPage::new(500.0, 700.0, 0, "Page 3", None),
            ],
        );
        let original = source_bytes(&source);
        let output = dir.path().join("selection.pdf");

        assert!(matches!(
            write_page_selection(&source, &output, &[], true),
            Err(SplitError::InvalidRanges(_))
        ));
        assert!(matches!(
            write_page_selection(&source, &output, &[1, 1], true),
            Err(SplitError::InvalidRanges(_))
        ));
        assert!(matches!(
            write_page_selection(&source, &output, &[4], true),
            Err(SplitError::InvalidRanges(_))
        ));

        fs::write(&output, b"keep me").unwrap();
        assert!(matches!(
            write_page_selection(&source, &output, &[1], false),
            Err(SplitError::OutputCollision { .. })
        ));
        assert_eq!(fs::read(&output).unwrap(), b"keep me");
        assert_eq!(source_bytes(&source), original);

        write_page_selection(&source, &output, &[2], true).unwrap();
        let document = Document::load(&output).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 1);
        assert!(page_text(&document, ids[0]).contains("Page 2"));
        assert_eq!(source_bytes(&source), original);

        assert!(matches!(
            write_page_selection(&source, &source, &[1], true),
            Err(SplitError::OutputMatchesInput { .. })
        ));
        assert_eq!(source_bytes(&source), original);
    }

    #[test]
    fn page_selection_preserves_images_fonts_and_inherited_resources() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("Image.pdf");
        let output = dir.path().join("selected.pdf");
        write_image_pdf(&source);

        write_page_selection(&source, &output, &[1], true).unwrap();
        let document = Document::load(&output).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 1);
        assert!(page_text(&document, ids[0]).contains("C1 image page"));
        assert_eq!(image_count(&document), 1);
        assert!(page_dictionary(&document, ids[0]).get(b"Resources").is_ok());
        assert!(page_dictionary(&document, ids[0]).get(b"MediaBox").is_ok());

        let nested = dir.path().join("Nested.pdf");
        let nested_output = dir.path().join("nested-selected.pdf");
        write_nested_inherited_pdf(&nested);
        write_page_selection(&nested, &nested_output, &[1], true).unwrap();
        let document = Document::load(&nested_output).unwrap();
        let id = page_ids(&document)[0];
        assert!(page_text(&document, id).contains("Nested inherited page"));
        assert_eq!(media_box(&document, id), (595.0, 842.0));
        assert!(page_dictionary(&document, id).get(b"Resources").is_ok());
    }
}
