//! Rotates selected pages by clockwise 90-degree increments.
//!
//! The operation updates page `/Rotate` values in a loaded document and saves
//! the whole PDF rather than rebuilding its page tree. Text, fonts, images,
//! vector content, annotations, links, and catalog-level structures are
//! therefore left for the normal PDF serialization path to preserve.
//!
//! PDF page rotation is display metadata. `/Rotate` values are resolved from
//! the page and its inherited page-tree ancestors, normalized modulo 360, and
//! required to be multiples of 90. Selected pages receive a direct `/Rotate`
//! value; unselected pages are not modified.
//!
//! Digital signatures are invalidated by re-saving a signed PDF. Encrypted
//! PDFs are rejected. Linearization and other serialization-level details may
//! change, but this module does not intentionally strip catalog content.
//!
//! Resource limits match the existing PDF operations: 512 MiB input, 2,000
//! pages, and a 256-level page-tree depth. The page count and depth are
//! enforced during an explicit tree walk so malformed trees cannot be expanded
//! without bound before validation.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId};
use tempfile::NamedTempFile;

/// Rotation uses the same input-size ceiling as the other PDF actions.
pub const MAX_ROTATE_INPUT_BYTES: u64 = 512 * 1024 * 1024;
/// Rotation uses the same page ceiling as Split, Extract, and Delete.
pub const MAX_ROTATE_PAGES: usize = 2_000;
/// Bounds recursive page-tree traversal. Valid PDFs are far below this.
const MAX_PAGE_TREE_DEPTH: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRotation {
    /// 1-based page number.
    pub page: usize,
    /// Number of clockwise 90-degree increments.
    pub turns: u8,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateSource {
    pub pages: usize,
    pub bytes: u64,
    /// Effective rotations (0/90/180/270) before any requested changes.
    pub rotations: Vec<u16>,
    pub has_signature: bool,
}

#[derive(Clone, Debug)]
pub struct RotateOutcome {
    pub pages: usize,
    pub rotated_pages: usize,
    pub bytes_written: u64,
}

#[derive(Debug)]
pub enum RotateError {
    InputNotFound { path: PathBuf },
    InputRead { path: PathBuf, source: io::Error },
    InputTooLarge { path: PathBuf, bytes: u64, max: u64 },
    InvalidPdf { path: PathBuf, reason: String },
    EncryptedPdf { path: PathBuf },
    InvalidReference { path: PathBuf, reference: String },
    TooManyPages { max: usize },
    EmptySelection,
    InvalidPage { page: usize, max: usize },
    DuplicatePage { page: usize },
    InvalidTurns { page: usize, turns: u8 },
    InvalidRotation { page: usize, value: i64 },
    OutputDirectoryInvalid { path: PathBuf },
    OutputMatchesInput { path: PathBuf },
    OutputCollision { path: PathBuf },
    OutputWrite { path: PathBuf, source: io::Error },
    OutputValidation { path: PathBuf, reason: String },
}

impl fmt::Display for RotateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputNotFound { path } => write!(formatter, "PDF not found: {}", path.display()),
            Self::InputRead { path, source } => {
                write!(formatter, "Could not read {}: {source}", path.display())
            }
            Self::InputTooLarge { path, bytes, max } => write!(
                formatter,
                "{} is too large to rotate ({:.0} MB). The limit is {:.0} MB.",
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
            Self::TooManyPages { max } => write!(
                formatter,
                "This PDF has too many pages to rotate. The limit is {max} pages."
            ),
            Self::EmptySelection => {
                write!(formatter, "Select at least one page to rotate.")
            }
            Self::InvalidPage { page, max } => write!(
                formatter,
                "Page {page} is outside this PDF. It has {max} {}.",
                if *max == 1 { "page" } else { "pages" }
            ),
            Self::DuplicatePage { page } => {
                write!(
                    formatter,
                    "Page {page} appears more than once in the selection."
                )
            }
            Self::InvalidTurns { page, turns } => write!(
                formatter,
                "Page {page} has an invalid rotation step ({turns}). Use 1 to 3 quarter turns."
            ),
            Self::InvalidRotation { page, value } => write!(
                formatter,
                "Page {page} has an unsupported /Rotate value ({value}). Rotation must be a multiple of 90."
            ),
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
            Self::OutputCollision { path } => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "That file".to_string());
                write!(
                    formatter,
                    "A file named \"{name}\" already exists. Choose a different name or folder."
                )
            }
            Self::OutputWrite { path, source } => {
                write!(formatter, "Could not write {}: {source}", path.display())
            }
            Self::OutputValidation { path, reason } => write!(
                formatter,
                "The generated PDF could not be validated: {}: {reason}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RotateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InputRead { source, .. } | Self::OutputWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn inspect_pdf(input: &Path) -> Result<RotateSource, RotateError> {
    let loaded = load_document(input)?;

    Ok(RotateSource {
        pages: loaded.pages.len(),
        bytes: fs::metadata(input)
            .map_err(|source| RotateError::InputRead {
                path: input.to_path_buf(),
                source,
            })?
            .len(),
        rotations: loaded.pages.iter().map(|page| page.rotation).collect(),
        has_signature: has_digital_signature(&loaded.document),
    })
}

/// Rotates the selected pages and writes a new PDF without overwriting an
/// existing output.
pub fn rotate_pdf(
    input: &Path,
    output: &Path,
    rotations: &[PageRotation],
) -> Result<RotateOutcome, RotateError> {
    rotate(input, output, rotations, false)
}

/// Like [`rotate_pdf`], but atomically replaces an existing output.
///
/// Use only after the user explicitly confirmed replacement in a native save
/// dialog.
pub fn rotate_pdf_with_overwrite(
    input: &Path,
    output: &Path,
    rotations: &[PageRotation],
) -> Result<RotateOutcome, RotateError> {
    rotate(input, output, rotations, true)
}

fn rotate(
    input: &Path,
    output: &Path,
    rotations: &[PageRotation],
    allow_overwrite: bool,
) -> Result<RotateOutcome, RotateError> {
    if rotations.is_empty() {
        return Err(RotateError::EmptySelection);
    }

    let mut loaded = load_document(input)?;
    validate_output(input, output, allow_overwrite)?;

    let page_count = loaded.pages.len();
    let page_ids = loaded.pages.iter().map(|page| page.id).collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let mut expected = loaded
        .pages
        .iter()
        .map(|page| page.rotation)
        .collect::<Vec<_>>();

    for change in rotations {
        if change.page == 0 || change.page > page_count {
            return Err(RotateError::InvalidPage {
                page: change.page,
                max: page_count,
            });
        }
        if !seen.insert(change.page) {
            return Err(RotateError::DuplicatePage { page: change.page });
        }
        if change.turns == 0 || change.turns > 3 {
            return Err(RotateError::InvalidTurns {
                page: change.page,
                turns: change.turns,
            });
        }

        let index = change.page - 1;
        let new_rotation =
            (i64::from(expected[index]) + 90 * i64::from(change.turns)).rem_euclid(360) as u16;
        expected[index] = new_rotation;

        let page = loaded
            .document
            .get_object_mut(page_ids[index])
            .and_then(Object::as_dict_mut)
            .map_err(|error| RotateError::InvalidPdf {
                path: input.to_path_buf(),
                reason: error.to_string(),
            })?;
        page.set("Rotate", Object::Integer(i64::from(new_rotation)));
    }

    let mut bytes = Vec::new();
    loaded
        .document
        .save_to(&mut bytes)
        .map_err(|error| RotateError::InvalidPdf {
            path: input.to_path_buf(),
            reason: error.to_string(),
        })?;

    write_safely(output, &bytes, allow_overwrite, &expected)?;

    Ok(RotateOutcome {
        pages: page_count,
        rotated_pages: rotations.len(),
        bytes_written: bytes.len() as u64,
    })
}

#[derive(Clone, Debug)]
struct PageEntry {
    id: ObjectId,
    rotation: u16,
}

struct LoadedDocument {
    document: Document,
    pages: Vec<PageEntry>,
}

fn load_document(path: &Path) -> Result<LoadedDocument, RotateError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(RotateError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(RotateError::InputRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if metadata.is_dir() {
        return Err(RotateError::InvalidPdf {
            path: path.to_path_buf(),
            reason: "path is a directory".to_string(),
        });
    }
    if metadata.len() > MAX_ROTATE_INPUT_BYTES {
        return Err(RotateError::InputTooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            max: MAX_ROTATE_INPUT_BYTES,
        });
    }

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(RotateError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(RotateError::InputRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let document = Document::load_mem(&bytes).map_err(|error| RotateError::InvalidPdf {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;

    if document.is_encrypted() {
        return Err(RotateError::EncryptedPdf {
            path: path.to_path_buf(),
        });
    }

    // Traverse the page tree before the general reference walk so malformed
    // or huge trees fail at the page-tree boundary instead of being fully
    // expanded first.
    let pages = page_entries(&document, path)?;
    validate_references(&document, path)?;

    Ok(LoadedDocument { document, pages })
}

fn page_entries(document: &Document, source: &Path) -> Result<Vec<PageEntry>, RotateError> {
    let root_id = document
        .catalog()
        .map_err(|error| RotateError::InvalidPdf {
            path: source.to_path_buf(),
            reason: error.to_string(),
        })?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| RotateError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "catalog has no page tree reference".to_string(),
        })?;

    let mut pages = Vec::new();
    let mut visited = HashSet::new();
    collect_page_entries(document, root_id, None, &mut visited, &mut pages, 0, source)?;

    if pages.is_empty() {
        return Err(RotateError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "document has no pages".to_string(),
        });
    }

    Ok(pages)
}

fn collect_page_entries(
    document: &Document,
    node_id: ObjectId,
    inherited_rotate: Option<Object>,
    visited: &mut HashSet<ObjectId>,
    pages: &mut Vec<PageEntry>,
    depth: usize,
    source: &Path,
) -> Result<(), RotateError> {
    if depth > MAX_PAGE_TREE_DEPTH {
        return Err(RotateError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "page tree is too deep".to_string(),
        });
    }
    if !visited.insert(node_id) {
        return Err(RotateError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "page tree contains a cycle or repeated node".to_string(),
        });
    }

    let node = document
        .get_object(node_id)
        .and_then(Object::as_dict)
        .map_err(|error| RotateError::InvalidPdf {
            path: source.to_path_buf(),
            reason: format!(
                "page-tree object {} is not a dictionary: {error}",
                node_id.0
            ),
        })?;

    let rotation = match node.get(b"Rotate") {
        Ok(value) => Some(value.clone()),
        Err(_) => inherited_rotate,
    };

    match node.get(b"Kids") {
        Ok(kids_object) => {
            let kids = resolve_object(document, kids_object)
                .and_then(|object| object.as_array().ok())
                .ok_or_else(|| RotateError::InvalidPdf {
                    path: source.to_path_buf(),
                    reason: format!("page-tree object {} has a non-array Kids entry", node_id.0),
                })?;

            for kid in kids {
                let kid_id = kid.as_reference().map_err(|_| RotateError::InvalidPdf {
                    path: source.to_path_buf(),
                    reason: "page-tree kid is not an indirect reference".to_string(),
                })?;
                collect_page_entries(
                    document,
                    kid_id,
                    rotation.clone(),
                    visited,
                    pages,
                    depth + 1,
                    source,
                )?;
            }
        }
        Err(_) => {
            if pages.len() >= MAX_ROTATE_PAGES {
                return Err(RotateError::TooManyPages {
                    max: MAX_ROTATE_PAGES,
                });
            }

            let page_number = pages.len() + 1;
            let rotation = normalized_rotation(document, rotation.as_ref(), page_number)?;
            pages.push(PageEntry {
                id: node_id,
                rotation,
            });
        }
    }

    Ok(())
}

fn normalized_rotation(
    document: &Document,
    value: Option<&Object>,
    page: usize,
) -> Result<u16, RotateError> {
    let Some(value) = value else {
        return Ok(0);
    };
    let value = resolve_object(document, value)
        .and_then(|object| object.as_i64().ok())
        .ok_or(RotateError::InvalidRotation { page, value: 0 })?;
    if value % 90 != 0 {
        return Err(RotateError::InvalidRotation { page, value });
    }
    Ok(value.rem_euclid(360) as u16)
}

fn resolve_object<'a>(document: &'a Document, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => document.get_object(*id).ok(),
        _ => Some(object),
    }
}

fn validate_references(document: &Document, source: &Path) -> Result<(), RotateError> {
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
                let target =
                    document
                        .get_object(id)
                        .map_err(|_| RotateError::InvalidReference {
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

fn validate_output(input: &Path, output: &Path, allow_overwrite: bool) -> Result<(), RotateError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    if !parent.is_dir() {
        return Err(RotateError::OutputDirectoryInvalid {
            path: parent.to_path_buf(),
        });
    }

    let parent = fs::canonicalize(parent).map_err(|_| RotateError::OutputDirectoryInvalid {
        path: parent.to_path_buf(),
    })?;
    let file_name = output
        .file_name()
        .ok_or_else(|| RotateError::OutputDirectoryInvalid {
            path: parent.clone(),
        })?;
    let candidate = parent.join(file_name);
    let input_canonical = fs::canonicalize(input).map_err(|_| RotateError::InvalidPdf {
        path: input.to_path_buf(),
        reason: "could not resolve the source path".to_string(),
    })?;
    let output_canonical = if output.exists() {
        fs::canonicalize(output).map_err(|_| RotateError::OutputDirectoryInvalid {
            path: output.to_path_buf(),
        })?
    } else {
        candidate
    };

    if output_canonical == input_canonical {
        return Err(RotateError::OutputMatchesInput {
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
            return Err(RotateError::OutputMatchesInput {
                path: output.to_path_buf(),
            });
        }
    }

    if output.exists() {
        if output.is_dir() {
            return Err(RotateError::OutputWrite {
                path: output.to_path_buf(),
                source: io::Error::new(io::ErrorKind::AlreadyExists, "output path is a directory"),
            });
        }
        if !allow_overwrite {
            return Err(RotateError::OutputCollision {
                path: output.to_path_buf(),
            });
        }
    }

    Ok(())
}

fn write_safely(
    target: &Path,
    bytes: &[u8],
    allow_overwrite: bool,
    expected_rotations: &[u16],
) -> Result<(), RotateError> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temp = NamedTempFile::new_in(parent).map_err(|source| RotateError::OutputWrite {
        path: target.to_path_buf(),
        source,
    })?;

    temp.write_all(bytes)
        .map_err(|source| RotateError::OutputWrite {
            path: target.to_path_buf(),
            source,
        })?;
    temp.as_file()
        .sync_all()
        .map_err(|source| RotateError::OutputWrite {
            path: target.to_path_buf(),
            source,
        })?;

    validate_written_pdf(temp.path(), target, expected_rotations)?;

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
                Err(RotateError::OutputCollision {
                    path: target.to_path_buf(),
                })
            } else {
                Err(RotateError::OutputWrite {
                    path: target.to_path_buf(),
                    source: error.error,
                })
            }
        }
    }
}

fn validate_written_pdf(
    temp: &Path,
    target: &Path,
    expected_rotations: &[u16],
) -> Result<(), RotateError> {
    let loaded = load_document(temp).map_err(|error| RotateError::OutputValidation {
        path: target.to_path_buf(),
        reason: error.to_string(),
    })?;

    if loaded.pages.len() != expected_rotations.len() {
        return Err(RotateError::OutputValidation {
            path: target.to_path_buf(),
            reason: format!(
                "expected {} pages, found {}",
                expected_rotations.len(),
                loaded.pages.len()
            ),
        });
    }

    for (index, (page, expected)) in loaded.pages.iter().zip(expected_rotations).enumerate() {
        if page.rotation != *expected {
            return Err(RotateError::OutputValidation {
                path: target.to_path_buf(),
                reason: format!(
                    "page {} expected rotation {expected}, found {}",
                    index + 1,
                    page.rotation
                ),
            });
        }
    }

    Ok(())
}

fn has_digital_signature(document: &Document) -> bool {
    if let Ok(catalog) = document.catalog()
        && let Ok(acro_form) = catalog.get(b"AcroForm")
        && let Some(dictionary) = resolve_dictionary(document, acro_form)
        && dictionary
            .get(b"SigFlags")
            .and_then(Object::as_i64)
            .is_ok_and(|flags| flags & 1 != 0)
    {
        return true;
    }

    document.objects.values().any(|object| {
        let Ok(dictionary) = object.as_dict() else {
            return false;
        };
        dictionary
            .get(b"Type")
            .and_then(Object::as_name)
            .is_ok_and(|name| name == b"Sig")
            || dictionary
                .get(b"FT")
                .and_then(Object::as_name)
                .is_ok_and(|name| name == b"Sig")
            || (dictionary.get(b"ByteRange").is_ok() && dictionary.get(b"Contents").is_ok())
    })
}

fn resolve_dictionary<'a>(document: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    if let Ok(id) = object.as_reference() {
        document.get_object(id).ok()?.as_dict().ok()
    } else {
        object.as_dict().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;
    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::{EncryptionState, EncryptionVersion, Permissions, Stream};
    use pdf_writer::types::{ActionType, AnnotationType};
    use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    struct TestPage {
        width: f32,
        height: f32,
        rotate: i32,
        text: &'static str,
    }

    fn flate(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
    }

    fn alpha_mask(width: u32, height: u32) -> Vec<u8> {
        let mut data = vec![0u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                data[(y * width + x) as usize] = ((x + y) % 255) as u8;
            }
        }
        data
    }

    fn write_fixture(path: &Path) {
        let pages = [
            TestPage {
                width: 612.0,
                height: 792.0,
                rotate: 0,
                text: "Page One",
            },
            TestPage {
                width: 500.0,
                height: 700.0,
                rotate: 90,
                text: "Page Two",
            },
            TestPage {
                width: 400.0,
                height: 300.0,
                rotate: 180,
                text: "Page Three",
            },
            TestPage {
                width: 300.0,
                height: 600.0,
                rotate: 270,
                text: "Page Four",
            },
        ];

        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let font = Ref::new(3);
        let info = Ref::new(4);
        let page_refs: Vec<Ref> = (0..pages.len())
            .map(|i| Ref::new(5 + i as i32 * 2))
            .collect();
        let content_refs: Vec<Ref> = (0..pages.len())
            .map(|i| Ref::new(6 + i as i32 * 2))
            .collect();
        let image = Ref::new(5 + pages.len() as i32 * 2);
        let smask = Ref::new(6 + pages.len() as i32 * 2);
        let link = Ref::new(7 + pages.len() as i32 * 2);

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree)
            .kids(page_refs.iter().copied())
            .count(pages.len() as i32);

        for (index, page) in pages.iter().enumerate() {
            let mut writer = pdf.page(page_refs[index]);
            writer.media_box(Rect::new(0.0, 0.0, page.width, page.height));
            writer.parent(tree);
            writer.contents(content_refs[index]);
            if page.rotate != 0 {
                writer.rotate(page.rotate);
            }
            {
                let mut resources = writer.resources();
                resources.fonts().pair(Name(b"F1"), font);
                if index == 0 {
                    resources.x_objects().pair(Name(b"Im1"), image);
                    resources.x_objects().pair(Name(b"Mask1"), smask);
                }
            }
            if index == 0 {
                writer.annotations([link]);
            }
        }

        pdf.type1_font(font).base_font(Name(b"Helvetica"));
        pdf.document_info(info)
            .title(TextStr("Rotation fixture"))
            .author(TextStr("Quicklet tests"))
            .producer(TextStr("Quicklet rotation proof"));

        let mask = alpha_mask(8, 8);
        let mask_data = flate(&mask);
        let mut writer = pdf.image_xobject(smask, &mask_data);
        writer.filter(Filter::FlateDecode);
        writer.width(8);
        writer.height(8);
        writer.color_space().device_gray();
        writer.bits_per_component(8);
        writer.finish();

        let mut pixels = Vec::with_capacity(8 * 8 * 3);
        for y in 0..8 {
            for x in 0..8 {
                pixels.push((x * 30) as u8);
                pixels.push((y * 30) as u8);
                pixels.push(160);
            }
        }
        let image_data = flate(&pixels);
        let mut writer = pdf.image_xobject(image, &image_data);
        writer.filter(Filter::FlateDecode);
        writer.width(8);
        writer.height(8);
        writer.color_space().device_rgb();
        writer.bits_per_component(8);
        writer.s_mask(smask);
        writer.finish();

        for (index, content_ref) in content_refs.iter().enumerate() {
            let mut content = Content::new();
            content.begin_text();
            content.set_font(Name(b"F1"), 20.0);
            content.next_line(72.0, 700.0);
            content.show(Str(pages[index].text.as_bytes()));
            content.end_text();
            content.rect(50.0, 100.0, 120.0, 80.0);
            content.stroke();
            if index == 0 {
                content.save_state();
                content.transform([80.0, 0.0, 0.0, 80.0, 150.0, 300.0]);
                content.x_object(Name(b"Im1"));
                content.restore_state();
            }
            pdf.stream(*content_ref, &content.finish());
        }

        let mut annotation = pdf.annotation(link);
        annotation.subtype(AnnotationType::Link);
        annotation.rect(Rect::new(72.0, 650.0, 280.0, 680.0));
        annotation
            .action()
            .action_type(ActionType::GoTo)
            .destination()
            .page(page_refs[1])
            .fit();
        annotation.finish();

        fs::write(path, pdf.finish()).unwrap();

        let mut document = Document::load(path).unwrap();
        let page_ids = page_entries(&document, Path::new("test"))
            .unwrap()
            .into_iter()
            .map(|page| page.id)
            .collect::<Vec<_>>();
        let pages_id = document
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();
        document
            .get_object_mut(pages_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Rotate", 90);

        {
            let page = document
                .get_object_mut(page_ids[2])
                .unwrap()
                .as_dict_mut()
                .unwrap();
            page.set("CropBox", vec![0.into(), 0.into(), 350.into(), 250.into()]);
        }
        {
            let page = document
                .get_object_mut(page_ids[3])
                .unwrap()
                .as_dict_mut()
                .unwrap();
            page.set("UserUnit", 2.0);
        }

        let catalog_id = document
            .trailer
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        let outlines_id = document.new_object_id();
        let item = document.add_object(dictionary! {
            "Title" => Object::string_literal("Chapter One"),
            "Parent" => outlines_id,
            "Dest" => vec![Object::Reference(page_ids[0]), Object::Name(b"Fit".to_vec())],
        });
        document.objects.insert(
            outlines_id,
            Object::Dictionary(dictionary! {
                "Type" => "Outlines",
                "First" => item,
                "Last" => item,
                "Count" => 1,
            }),
        );

        let field = document.add_object(dictionary! {
            "FT" => "Tx",
            "T" => Object::string_literal("Name"),
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => vec![44.into(), 500.into(), 244.into(), 520.into()],
            "P" => page_ids[0],
            "F" => 4,
            "V" => Object::string_literal("Test value"),
        });
        let acro_form = document.add_object(dictionary! {
            "Fields" => vec![Object::Reference(field)],
            "SigFlags" => 0,
        });
        let struct_tree = document.add_object(dictionary! {
            "Type" => "StructTreeRoot",
            "K" => Vec::<Object>::new(),
        });
        let metadata = document.add_object(Stream::new(
            dictionary! {
                "Type" => "Metadata",
                "Subtype" => "XML",
            },
            b"<x:xmpmeta>Quicklet rotation metadata</x:xmpmeta>".to_vec(),
        ));

        {
            let page = document
                .get_object_mut(page_ids[0])
                .unwrap()
                .as_dict_mut()
                .unwrap();
            page.get_mut(b"Annots")
                .unwrap()
                .as_array_mut()
                .unwrap()
                .push(Object::Reference(field));
        }

        {
            let catalog = document
                .get_object_mut(catalog_id)
                .unwrap()
                .as_dict_mut()
                .unwrap();
            catalog.set("Outlines", outlines_id);
            catalog.set("AcroForm", acro_form);
            catalog.set("StructTreeRoot", struct_tree);
            catalog.set("MarkInfo", dictionary! { "Marked" => true });
            catalog.set("Metadata", metadata);
            catalog.set(
                "PageLabels",
                dictionary! {
                    "Nums" => vec![0.into(), dictionary! { "S" => "D" }.into()],
                },
            );
        }

        document.save(path).unwrap();
    }

    fn add_signature(path: &Path) {
        let mut document = Document::load(path).unwrap();
        let page_ids = page_entries(&document, Path::new("test"))
            .unwrap()
            .into_iter()
            .map(|page| page.id)
            .collect::<Vec<_>>();
        let catalog_id = document
            .trailer
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        let signature_value = document.add_object(dictionary! {
            "Type" => "Sig",
            "ByteRange" => vec![0.into(), 0.into(), 0.into(), 0.into()],
            "Contents" => Object::string_literal("signature"),
        });
        let field = document.add_object(dictionary! {
            "FT" => "Sig",
            "T" => Object::string_literal("Signature1"),
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => vec![44.into(), 540.into(), 244.into(), 560.into()],
            "P" => page_ids[0],
            "V" => signature_value,
        });
        let acro_form = document.add_object(dictionary! {
            "Fields" => vec![Object::Reference(field)],
            "SigFlags" => 1,
        });
        let catalog = document
            .get_object_mut(catalog_id)
            .unwrap()
            .as_dict_mut()
            .unwrap();
        catalog.set("AcroForm", acro_form);
        document.save(path).unwrap();
    }

    fn write_encrypted_pdf(source: &Path, output: &Path) {
        let mut document = Document::load(source).unwrap();
        let file_id = Object::string_literal(b"quicklet-rotate-test");
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
        page_entries(document, Path::new("test"))
            .unwrap()
            .into_iter()
            .map(|page| page.id)
            .collect()
    }

    fn page_dictionary(document: &Document, id: ObjectId) -> &Dictionary {
        document.get_object(id).unwrap().as_dict().unwrap()
    }

    fn media_box(document: &Document, id: ObjectId) -> (f32, f32) {
        let media = page_dictionary(document, id)
            .get(b"MediaBox")
            .unwrap()
            .as_array()
            .unwrap();
        (media[2].as_float().unwrap(), media[3].as_float().unwrap())
    }

    fn crop_box(document: &Document, id: ObjectId) -> (f32, f32) {
        let crop = page_dictionary(document, id)
            .get(b"CropBox")
            .unwrap()
            .as_array()
            .unwrap();
        (crop[2].as_float().unwrap(), crop[3].as_float().unwrap())
    }

    fn effective(document: &Document, id: ObjectId, _page: usize) -> u16 {
        page_entries(document, Path::new("test"))
            .unwrap()
            .into_iter()
            .find(|page| page.id == id)
            .unwrap()
            .rotation
    }

    fn page_text(document: &Document, id: ObjectId) -> String {
        String::from_utf8_lossy(&document.get_page_content(id)).into_owned()
    }

    fn link_targets(document: &Document, id: ObjectId) -> Vec<u32> {
        let page_numbers: HashMap<ObjectId, u32> = page_entries(document, Path::new("test"))
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, page)| (page.id, index as u32 + 1))
            .collect();
        let mut targets = Vec::new();
        let Ok(annots) = page_dictionary(document, id)
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
            let Some(first) = array.first() else {
                continue;
            };
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

    fn font_count(document: &Document) -> usize {
        document
            .objects
            .values()
            .filter(|object| object.type_name().is_ok_and(|name| name == b"Font"))
            .count()
    }

    fn annotation_count(document: &Document) -> usize {
        page_ids(document)
            .iter()
            .filter_map(|id| {
                page_dictionary(document, *id)
                    .get(b"Annots")
                    .ok()
                    .and_then(|value| value.as_array().ok())
                    .map(Vec::len)
            })
            .sum()
    }

    fn info_value(document: &Document, key: &[u8]) -> Option<Vec<u8>> {
        let info = document.trailer.get(b"Info").ok()?;
        let dictionary = if let Ok(id) = info.as_reference() {
            document
                .get_object(id)
                .ok()
                .and_then(|object| object.as_dict().ok())
        } else {
            info.as_dict().ok()
        }?;
        dictionary.get(key).ok()?.as_str().ok().map(Vec::from)
    }

    fn source_bytes(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()
    }

    fn write_many_pages(path: &Path, count: usize) {
        let mut document = Document::with_version("1.5");
        let root = document.new_object_id();
        let mut kids = Vec::with_capacity(count);
        for _ in 0..count {
            let page = document.new_object_id();
            document.objects.insert(
                page,
                Object::Dictionary(dictionary! {
                    "Type" => "Page",
                    "Parent" => root,
                    "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
                }),
            );
            kids.push(Object::Reference(page));
        }
        document.objects.insert(
            root,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count as i64,
            }),
        );
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => root,
        });
        document.trailer.set("Root", catalog);
        document.save(path).unwrap();
    }

    fn write_deep_page_tree(path: &Path) {
        let mut document = Document::with_version("1.5");
        let root = document.new_object_id();
        let mut current = root;
        for _ in 0..MAX_PAGE_TREE_DEPTH + 4 {
            let child = document.new_object_id();
            document.objects.insert(
                current,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages",
                    "Kids" => vec![Object::Reference(child)],
                    "Count" => 1,
                }),
            );
            current = child;
        }
        let page = document.new_object_id();
        document.objects.insert(
            page,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => current,
                "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            }),
        );
        document.objects.insert(
            current,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page)],
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

    fn write_page_tree(path: &Path, kids: Object) {
        let mut document = Document::with_version("1.5");
        let root = document.new_object_id();
        document.objects.insert(
            root,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
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

    #[test]
    fn inspects_direct_and_inherited_rotations() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        write_fixture(&source);

        let info = inspect_pdf(&source).unwrap();
        assert_eq!(info.pages, 4);
        assert_eq!(info.rotations, vec![90, 90, 180, 270]);
        assert!(!info.has_signature);
        assert_eq!(source_bytes(&source).len() as u64, info.bytes);
    }

    #[test]
    fn rotates_selected_pages_and_preserves_document_content() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let output = dir.path().join("rotated.pdf");
        write_fixture(&source);
        let original = source_bytes(&source);
        let source_document = Document::load(&source).unwrap();
        let source_ids = page_ids(&source_document);
        let source_content = source_ids
            .iter()
            .map(|id| page_text(&source_document, *id))
            .collect::<Vec<_>>();

        let outcome = rotate_pdf(
            &source,
            &output,
            &[
                PageRotation { page: 1, turns: 1 },
                PageRotation { page: 2, turns: 2 },
                PageRotation { page: 4, turns: 1 },
            ],
        )
        .unwrap();
        assert_eq!(outcome.pages, 4);
        assert_eq!(outcome.rotated_pages, 3);
        assert_eq!(source_bytes(&source), original);

        let document = Document::load(&output).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 4);
        assert_eq!(
            ids.iter()
                .enumerate()
                .map(|(index, id)| effective(&document, *id, index + 1))
                .collect::<Vec<_>>(),
            vec![180, 270, 180, 0]
        );
        assert_eq!(media_box(&document, ids[0]), (612.0, 792.0));
        assert_eq!(crop_box(&document, ids[2]), (350.0, 250.0));
        assert_eq!(
            page_dictionary(&document, ids[3])
                .get(b"UserUnit")
                .unwrap()
                .as_float()
                .unwrap(),
            2.0
        );
        assert!(page_text(&document, ids[0]).contains("Page One"));
        assert_eq!(font_count(&document), font_count(&source_document));
        assert_eq!(image_count(&document), image_count(&source_document));
        assert_eq!(
            annotation_count(&document),
            annotation_count(&source_document)
        );
        assert_eq!(link_targets(&document, ids[0]), vec![2]);
        assert_eq!(
            ids.iter()
                .map(|id| page_text(&document, *id))
                .collect::<Vec<_>>(),
            source_content
        );
        assert_eq!(
            info_value(&document, b"Title").as_deref(),
            Some(b"Rotation fixture".as_slice())
        );

        let catalog = document.catalog().unwrap();
        for key in [
            b"Outlines".as_slice(),
            b"AcroForm",
            b"StructTreeRoot",
            b"MarkInfo",
            b"Metadata",
            b"PageLabels",
        ] {
            assert!(catalog.get(key).is_ok(), "missing catalog {key:?}");
        }

        let outlines_id = catalog.get(b"Outlines").unwrap().as_reference().unwrap();
        let outlines = document.get_object(outlines_id).unwrap().as_dict().unwrap();
        let outline_item = document
            .get_object(outlines.get(b"First").unwrap().as_reference().unwrap())
            .unwrap()
            .as_dict()
            .unwrap();
        assert_eq!(
            outline_item.get(b"Title").unwrap().as_str().unwrap(),
            b"Chapter One"
        );

        let acro_id = catalog.get(b"AcroForm").unwrap().as_reference().unwrap();
        let acro = document.get_object(acro_id).unwrap().as_dict().unwrap();
        assert_eq!(acro.get(b"Fields").unwrap().as_array().unwrap().len(), 1);

        let metadata_id = catalog.get(b"Metadata").unwrap().as_reference().unwrap();
        let metadata = document
            .get_object(metadata_id)
            .unwrap()
            .as_stream()
            .unwrap();
        assert!(String::from_utf8_lossy(&metadata.content).contains("Quicklet rotation metadata"));

        let labels = catalog.get(b"PageLabels").unwrap().as_dict().unwrap();
        assert_eq!(labels.get(b"Nums").unwrap().as_array().unwrap().len(), 2);

        let struct_id = catalog
            .get(b"StructTreeRoot")
            .unwrap()
            .as_reference()
            .unwrap();
        assert_eq!(
            document
                .get_object(struct_id)
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"Type")
                .unwrap()
                .as_name()
                .unwrap(),
            b"StructTreeRoot"
        );
    }

    #[test]
    fn four_quarter_turns_return_to_the_original_orientation() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        write_fixture(&source);
        let original = source_bytes(&source);
        let source_document = Document::load(&source).unwrap();
        let original_rotation = inspect_pdf(&source).unwrap().rotations[0];
        let original_content = page_text(&source_document, page_ids(&source_document)[0]);

        let mut current = source.clone();
        for step in 0..4 {
            let output = dir.path().join(format!("step-{step}.pdf"));
            rotate_pdf(&current, &output, &[PageRotation { page: 1, turns: 1 }]).unwrap();
            current = output;
        }

        let document = Document::load(&current).unwrap();
        let id = page_ids(&document)[0];
        assert_eq!(effective(&document, id, 1), original_rotation);
        assert_eq!(page_text(&document, id), original_content);
        assert_eq!(source_bytes(&source), original);
    }

    #[test]
    fn rejects_invalid_selections_rotations_and_inputs() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let output = dir.path().join("output.pdf");
        write_fixture(&source);

        assert!(matches!(
            rotate_pdf(&source, &output, &[]),
            Err(RotateError::EmptySelection)
        ));
        for page in [0, 5] {
            assert!(matches!(
                rotate_pdf(&source, &output, &[PageRotation { page, turns: 1 }]),
                Err(RotateError::InvalidPage { .. })
            ));
        }
        assert!(matches!(
            rotate_pdf(
                &source,
                &output,
                &[
                    PageRotation { page: 1, turns: 1 },
                    PageRotation { page: 1, turns: 2 }
                ]
            ),
            Err(RotateError::DuplicatePage { page: 1 })
        ));
        for turns in [0, 4] {
            assert!(matches!(
                rotate_pdf(&source, &output, &[PageRotation { page: 1, turns }]),
                Err(RotateError::InvalidTurns {
                    page: 1,
                    turns: 0 | 4
                })
            ));
        }

        let bad_rotation = dir.path().join("bad-rotation.pdf");
        let mut document = Document::load(&source).unwrap();
        let page_id = page_ids(&document)[1];
        document
            .get_object_mut(page_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Rotate", 45);
        document.save(&bad_rotation).unwrap();
        assert!(matches!(
            inspect_pdf(&bad_rotation),
            Err(RotateError::InvalidRotation { page: 2, value: 45 })
        ));
        assert!(
            rotate_pdf(
                &bad_rotation,
                &output,
                &[PageRotation { page: 1, turns: 1 }]
            )
            .is_err()
        );

        let encrypted = dir.path().join("encrypted.pdf");
        write_encrypted_pdf(&source, &encrypted);
        assert!(matches!(
            inspect_pdf(&encrypted),
            Err(RotateError::EncryptedPdf { .. })
        ));

        let malformed = dir.path().join("malformed.pdf");
        let bytes = source_bytes(&source);
        fs::write(&malformed, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            inspect_pdf(&malformed),
            Err(RotateError::InvalidPdf { .. })
        ));

        let broken = dir.path().join("broken.pdf");
        let mut document = Document::load(&source).unwrap();
        let page_id = page_ids(&document)[0];
        document
            .get_object_mut(page_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Contents", Object::Reference((9999, 0)));
        document.save(&broken).unwrap();
        assert!(matches!(
            inspect_pdf(&broken),
            Err(RotateError::InvalidReference { .. })
        ));
        assert!(!output.exists());
    }

    #[test]
    fn rejects_source_overwrite_and_collisions_and_supports_explicit_overwrite() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let output = dir.path().join("output.pdf");
        write_fixture(&source);
        let original = source_bytes(&source);

        assert!(matches!(
            rotate_pdf(&source, &source, &[PageRotation { page: 1, turns: 1 }]),
            Err(RotateError::OutputMatchesInput { .. })
        ));
        assert_eq!(source_bytes(&source), original);

        fs::write(&output, b"keep me").unwrap();
        assert!(matches!(
            rotate_pdf(&source, &output, &[PageRotation { page: 1, turns: 1 }]),
            Err(RotateError::OutputCollision { .. })
        ));
        assert_eq!(source_bytes(&output), b"keep me");
        rotate_pdf_with_overwrite(&source, &output, &[PageRotation { page: 1, turns: 1 }]).unwrap();
        assert_ne!(source_bytes(&output), b"keep me");
        assert_eq!(source_bytes(&source), original);

        #[cfg(unix)]
        {
            let linked = dir.path().join("linked.pdf");
            fs::hard_link(&source, &linked).unwrap();
            assert!(matches!(
                rotate_pdf(&source, &linked, &[PageRotation { page: 1, turns: 1 }]),
                Err(RotateError::OutputMatchesInput { .. })
            ));
        }
    }

    #[test]
    fn validates_temporary_output_and_cleans_up_on_failure() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("invalid.pdf");
        let error = write_safely(&target, b"not a pdf", false, &[0]).unwrap_err();
        assert!(matches!(error, RotateError::OutputValidation { .. }));
        assert!(!target.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn detects_signed_pdfs() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        write_fixture(&source);
        assert!(!inspect_pdf(&source).unwrap().has_signature);
        add_signature(&source);
        assert!(inspect_pdf(&source).unwrap().has_signature);
    }

    #[test]
    fn hardens_page_tree_traversal_and_page_limits() {
        let dir = tempdir().unwrap();
        let accepted = dir.path().join("accepted-2000.pdf");
        write_many_pages(&accepted, MAX_ROTATE_PAGES);
        let info = inspect_pdf(&accepted).unwrap();
        assert_eq!(info.pages, MAX_ROTATE_PAGES);
        assert_eq!(info.rotations.len(), MAX_ROTATE_PAGES);
        assert!(info.rotations.iter().all(|rotation| *rotation == 0));

        let rejected = dir.path().join("rejected-2001.pdf");
        write_many_pages(&rejected, MAX_ROTATE_PAGES + 1);
        assert!(matches!(
            inspect_pdf(&rejected),
            Err(RotateError::TooManyPages {
                max: MAX_ROTATE_PAGES
            })
        ));

        let deep = dir.path().join("deep.pdf");
        write_deep_page_tree(&deep);
        assert!(matches!(
            inspect_pdf(&deep),
            Err(RotateError::InvalidPdf { .. })
        ));

        let missing_kid = dir.path().join("missing-kid.pdf");
        write_page_tree(
            &missing_kid,
            Object::Array(vec![Object::Reference((9999, 0))]),
        );
        assert!(matches!(
            inspect_pdf(&missing_kid),
            Err(RotateError::InvalidPdf { .. })
        ));

        let non_array_kids = dir.path().join("non-array-kids.pdf");
        write_page_tree(&non_array_kids, Object::Integer(1));
        assert!(matches!(
            inspect_pdf(&non_array_kids),
            Err(RotateError::InvalidPdf { .. })
        ));

        let cycle = dir.path().join("cycle.pdf");
        let mut document = Document::with_version("1.5");
        let root = document.new_object_id();
        document.objects.insert(
            root,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(root)],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => root,
        });
        document.trailer.set("Root", catalog);
        document.save(&cycle).unwrap();
        assert!(matches!(
            inspect_pdf(&cycle),
            Err(RotateError::InvalidPdf { .. })
        ));
    }

    #[test]
    fn enforces_input_size_and_missing_output_directory() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        write_fixture(&source);

        let huge = dir.path().join("huge.pdf");
        let file = fs::File::create(&huge).unwrap();
        file.set_len(MAX_ROTATE_INPUT_BYTES + 1).unwrap();
        assert!(matches!(
            inspect_pdf(&huge),
            Err(RotateError::InputTooLarge {
                max: MAX_ROTATE_INPUT_BYTES,
                ..
            })
        ));

        let missing = dir.path().join("missing").join("output.pdf");
        assert!(matches!(
            rotate_pdf(&source, &missing, &[PageRotation { page: 1, turns: 1 }]),
            Err(RotateError::OutputDirectoryInvalid { .. })
        ));
        assert!(!missing.exists());
    }
}
