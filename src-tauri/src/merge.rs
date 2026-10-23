//! Merges complete PDF files into a single document.
//!
//! Supported: page order, page dimensions, page rotation, vector content,
//! fonts, images, inherited page resources, and link annotations with direct
//! page destinations.
//!
//! Not preserved: bookmarks/outlines, named destinations, AcroForm/XFA fields,
//! tagged-PDF structure, optional content groups, page labels, document-level
//! metadata, and digital signatures. Encrypted PDFs are rejected.
//!
//! The merge rebuilds the page tree and catalog, so catalog-level structures
//! from the inputs are intentionally not carried over.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId};
use tempfile::NamedTempFile;

pub const MAX_MERGE_INPUTS: usize = 8;
const MAX_PAGE_TREE_DEPTH: usize = 256;

#[derive(Debug)]
pub enum MergeError {
    TooFewInputs,
    TooManyInputs { max: usize },
    InputNotFound { path: PathBuf },
    InputRead { path: PathBuf, source: io::Error },
    InvalidPdf { path: PathBuf, reason: String },
    EncryptedPdf { path: PathBuf },
    InvalidReference { path: PathBuf, reference: String },
    OutputExists { path: PathBuf },
    OutputDirectoryInvalid { path: PathBuf },
    OutputWrite { path: PathBuf, source: io::Error },
}

impl fmt::Display for MergeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewInputs => write!(formatter, "Select at least two PDF files to merge."),
            Self::TooManyInputs { max } => {
                write!(formatter, "At most {max} PDF files can be merged.")
            }
            Self::InputNotFound { path } => {
                write!(formatter, "PDF not found: {}", path.display())
            }
            Self::InputRead { path, source } => {
                write!(formatter, "Could not read {}: {source}", path.display())
            }
            Self::InvalidPdf { path, reason } => {
                write!(formatter, "Invalid PDF {}: {reason}", path.display())
            }
            Self::EncryptedPdf { path } => {
                write!(formatter, "Encrypted PDFs are not supported: {}", path.display())
            }
            Self::InvalidReference { path, reference } => {
                write!(
                    formatter,
                    "Invalid reference in {}: object {reference} is missing.",
                    path.display()
                )
            }
            Self::OutputExists { path } => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "That file".to_string());
                write!(
                    formatter,
                    "A file named \"{name}\" already exists. Choose a different name or folder."
                )
            }
            Self::OutputDirectoryInvalid { path } => {
                write!(
                    formatter,
                    "Output folder does not exist: {}",
                    path.display()
                )
            }
            Self::OutputWrite { path, source } => {
                write!(formatter, "Could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for MergeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InputRead { source, .. } | Self::OutputWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MergeOutcome {
    pub pages: usize,
    pub bytes_written: u64,
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

/// Merges complete PDFs in the given order. Existing output files are never
/// overwritten.
pub fn merge_pdfs(inputs: &[PathBuf], output: &Path) -> Result<MergeOutcome, MergeError> {
    merge(inputs, output, false)
}

/// Like [`merge_pdfs`], but atomically replaces an existing output file.
///
/// Use this only after the user explicitly confirmed replacement in a native
/// save dialog.
pub fn merge_pdfs_with_overwrite(
    inputs: &[PathBuf],
    output: &Path,
) -> Result<MergeOutcome, MergeError> {
    merge(inputs, output, true)
}

fn merge(
    inputs: &[PathBuf],
    output: &Path,
    allow_overwrite: bool,
) -> Result<MergeOutcome, MergeError> {
    if inputs.len() < 2 {
        return Err(MergeError::TooFewInputs);
    }
    if inputs.len() > MAX_MERGE_INPUTS {
        return Err(MergeError::TooManyInputs {
            max: MAX_MERGE_INPUTS,
        });
    }

    validate_output(output, allow_overwrite)?;

    let mut documents = Vec::with_capacity(inputs.len());
    for path in inputs {
        documents.push(load_document(path)?);
    }

    let mut merged = Document::with_version("1.7");
    let mut next_id = 1u32;
    let mut page_ids: Vec<ObjectId> = Vec::new();

    for (mut document, source) in documents {
        validate_references(&document, &source)?;
        document.renumber_objects_with(next_id);
        next_id = document.max_id + 1;

        let pages = flatten_page_tree(&mut document, &source)?;

        let page_set: HashSet<ObjectId> = pages.iter().copied().collect();
        page_ids.extend(pages);

        for (id, object) in document.objects {
            if page_set.contains(&id) {
                merged.objects.insert(id, object);
            } else {
                let type_name = object.type_name().unwrap_or(b"");
                if type_name != b"Catalog" && type_name != b"Pages" {
                    merged.objects.insert(id, object);
                }
            }
        }
    }

    merged.max_id = next_id;
    let new_pages_id = merged.new_object_id();
    let new_catalog_id = merged.new_object_id();

    for page_id in &page_ids {
        if let Ok(page) = merged
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
    merged
        .objects
        .insert(new_pages_id, Object::Dictionary(pages_dictionary));

    let mut catalog = Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", new_pages_id);
    merged
        .objects
        .insert(new_catalog_id, Object::Dictionary(catalog));
    merged.trailer.set("Root", new_catalog_id);

    validate_references(&merged, Path::new("merged output"))?;

    let mut bytes = Vec::new();
    merged
        .save_to(&mut bytes)
        .map_err(|error| MergeError::InvalidPdf {
            path: output.to_path_buf(),
            reason: error.to_string(),
        })?;

    write_safely(output, &bytes, allow_overwrite)?;

    Ok(MergeOutcome {
        pages: page_ids.len(),
        bytes_written: bytes.len() as u64,
    })
}

fn load_document(path: &Path) -> Result<(Document, PathBuf), MergeError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(MergeError::InputNotFound {
                path: path.to_path_buf(),
            })
        }
        Err(source) => {
            return Err(MergeError::InputRead {
                path: path.to_path_buf(),
                source,
            })
        }
    };

    let document = Document::load_mem(&bytes).map_err(|error| MergeError::InvalidPdf {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;

    if document.is_encrypted() {
        return Err(MergeError::EncryptedPdf {
            path: path.to_path_buf(),
        });
    }

    Ok((document, path.to_path_buf()))
}

fn flatten_page_tree(
    document: &mut Document,
    source: &Path,
) -> Result<Vec<ObjectId>, MergeError> {
    let root_id = document
        .catalog()
        .map_err(|error| MergeError::InvalidPdf {
            path: source.to_path_buf(),
            reason: error.to_string(),
        })?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| MergeError::InvalidPdf {
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
        return Err(MergeError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "document has no pages".to_string(),
        });
    }

    for (page_id, inherited) in &pages {
        let page = document
            .get_object_mut(*page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|error| MergeError::InvalidPdf {
                path: source.to_path_buf(),
                reason: error.to_string(),
            })?;

        for (key, value) in [
            (b"Resources".as_slice(), inherited.resources.as_ref()),
            (b"MediaBox".as_slice(), inherited.media_box.as_ref()),
            (b"CropBox".as_slice(), inherited.crop_box.as_ref()),
            (b"Rotate".as_slice(), inherited.rotate.as_ref()),
        ] {
            if page.get(key).is_err() {
                if let Some(value) = value {
                    page.set(key, value.clone());
                }
            }
        }
    }

    Ok(pages.into_iter().map(|(id, _)| id).collect())
}

fn collect_pages(
    document: &Document,
    node_id: ObjectId,
    inherited: &InheritedAttributes,
    visited: &mut HashSet<ObjectId>,
    pages: &mut Vec<(ObjectId, InheritedAttributes)>,
    depth: usize,
    source: &Path,
) -> Result<(), MergeError> {
    if depth > MAX_PAGE_TREE_DEPTH {
        return Err(MergeError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "page tree is too deep or cyclic".to_string(),
        });
    }
    if !visited.insert(node_id) {
        return Err(MergeError::InvalidPdf {
            path: source.to_path_buf(),
            reason: "page tree contains a cycle".to_string(),
        });
    }

    let node = document
        .get_object(node_id)
        .and_then(Object::as_dict)
        .map_err(|error| MergeError::InvalidPdf {
            path: source.to_path_buf(),
            reason: error.to_string(),
        })?;

    let current = inherited.merged_with(node);

    if let Ok(kids) = node.get(b"Kids").and_then(Object::as_array) {
        for kid in kids {
            let kid_id = kid.as_reference().map_err(|_| MergeError::InvalidPdf {
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
        pages.push((node_id, current));
    }

    Ok(())
}

fn validate_references(document: &Document, source: &Path) -> Result<(), MergeError> {
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
                let target = document.get_object(id).map_err(|_| {
                    MergeError::InvalidReference {
                        path: source.to_path_buf(),
                        reference: format!("{} {}", id.0, id.1),
                    }
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

fn validate_output(output: &Path, allow_overwrite: bool) -> Result<(), MergeError> {
    if output.exists() {
        if output.is_dir() {
            return Err(MergeError::OutputWrite {
                path: output.to_path_buf(),
                source: io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "output path is a directory",
                ),
            });
        }
        if !allow_overwrite {
            return Err(MergeError::OutputExists {
                path: output.to_path_buf(),
            });
        }
    }

    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    if !parent.is_dir() {
        return Err(MergeError::OutputDirectoryInvalid {
            path: parent.to_path_buf(),
        });
    }

    Ok(())
}

fn write_safely(
    output: &Path,
    bytes: &[u8],
    allow_overwrite: bool,
) -> Result<(), MergeError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temp = NamedTempFile::new_in(parent).map_err(|source| MergeError::OutputWrite {
        path: output.to_path_buf(),
        source,
    })?;

    temp.write_all(bytes)
        .map_err(|source| MergeError::OutputWrite {
            path: output.to_path_buf(),
            source,
        })?;
    temp.as_file()
        .sync_all()
        .map_err(|source| MergeError::OutputWrite {
            path: output.to_path_buf(),
            source,
        })?;

    let persisted = if allow_overwrite {
        temp.persist(output)
    } else {
        temp.persist_noclobber(output)
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
                Err(MergeError::OutputExists {
                    path: output.to_path_buf(),
                })
            } else {
                Err(MergeError::OutputWrite {
                    path: output.to_path_buf(),
                    source: error.error,
                })
            }
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
        pdf.pages(tree)
            .kids(page_refs.iter().copied())
            .count(count);

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
            if let (Some(annotation), Some(target)) =
                (annotation_refs[index], page.link_to)
            {
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
        let file_id = Object::string_literal(b"quicklet-merge-test");
        document
            .trailer
            .set("ID", vec![file_id.clone(), file_id]);

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

    fn page_dictionary<'a>(document: &'a Document, page_id: ObjectId) -> &'a Dictionary {
        document.get_object(page_id).unwrap().as_dict().unwrap()
    }

    fn media_box(document: &Document, page_id: ObjectId) -> (f32, f32) {
        let page = page_dictionary(document, page_id);
        let media = page.get(b"MediaBox").unwrap().as_array().unwrap();
        (
            media[2].as_float().unwrap(),
            media[3].as_float().unwrap(),
        )
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
            let Some(dictionary) = dictionary else { continue };
            let destination = dictionary
                .get(b"A")
                .ok()
                .and_then(|action| action.as_dict().ok())
                .and_then(|action| action.get(b"D").ok())
                .or_else(|| dictionary.get(b"Dest").ok());
            let Some(destination) = destination else { continue };
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
                    .and_then(|stream| {
                        stream
                            .dict
                            .get(b"Subtype")
                            .and_then(Object::as_name)
                            .ok()
                    })
                    .is_some_and(|name| name == b"Image")
            })
            .count()
    }

    #[test]
    fn merges_in_order_and_preserves_pages_resources_and_links() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        let c = dir.path().join("c.pdf");
        let output = dir.path().join("merged.pdf");

        write_test_pdf(
            &a,
            &[
                TestPage::new(595.0, 842.0, 90, "A1 rotated page", None),
                TestPage::new(612.0, 792.0, 0, "A2 normal page", Some(0)),
            ],
        );
        write_test_pdf(
            &b,
            &[
                TestPage::new(595.0, 842.0, 0, "B1 front page", Some(1)),
                TestPage::new(792.0, 612.0, 0, "B2 landscape page", None),
            ],
        );
        write_image_pdf(&c);

        let outcome = merge_pdfs(&[a, b, c], &output).unwrap();
        assert_eq!(outcome.pages, 5);

        let document = Document::load(&output).unwrap();
        let pages = page_ids(&document);
        assert_eq!(pages.len(), 5);

        let texts: Vec<String> = pages
            .iter()
            .map(|page| page_text(&document, *page))
            .collect();
        for (text, expected) in texts.iter().zip(["A1", "A2", "B1", "B2", "C1"]) {
            assert!(text.contains(expected), "missing {expected} in {text}");
        }

        assert_eq!(media_box(&document, pages[0]), (595.0, 842.0));
        assert_eq!(rotation(&document, pages[0]), 90);
        assert_eq!(media_box(&document, pages[1]), (612.0, 792.0));
        assert_eq!(media_box(&document, pages[3]), (792.0, 612.0));
        assert_eq!(media_box(&document, pages[4]), (300.0, 200.0));

        assert_eq!(link_targets(&document, pages[1]), vec![1]);
        assert_eq!(link_targets(&document, pages[2]), vec![4]);
        assert!(image_count(&document) >= 1);
    }

    #[test]
    fn supports_nested_page_trees_with_inherited_resources() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nested.pdf");
        let other = dir.path().join("other.pdf");
        let output = dir.path().join("merged.pdf");
        write_nested_inherited_pdf(&nested);
        write_test_pdf(
            &other,
            &[TestPage::new(595.0, 842.0, 0, "Other page", None)],
        );

        merge_pdfs(&[nested, other], &output).unwrap();
        let document = Document::load(&output).unwrap();
        let pages = page_ids(&document);
        assert_eq!(pages.len(), 2);

        let page = page_dictionary(&document, pages[0]);
        assert!(page.get(b"Resources").is_ok());
        assert!(page.get(b"MediaBox").is_ok());
        assert!(page_text(&document, pages[0]).contains("Nested inherited page"));
    }

    #[test]
    fn merges_duplicate_paths_without_id_collisions() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let output = dir.path().join("merged.pdf");
        write_test_pdf(
            &source,
            &[
                TestPage::new(595.0, 842.0, 0, "First page", None),
                TestPage::new(595.0, 842.0, 0, "Second page", Some(0)),
            ],
        );

        let outcome = merge_pdfs(&[source.clone(), source], &output).unwrap();
        assert_eq!(outcome.pages, 4);

        let document = Document::load(&output).unwrap();
        let pages = page_ids(&document);
        assert_eq!(link_targets(&document, pages[1]), vec![1]);
        assert_eq!(link_targets(&document, pages[3]), vec![3]);
    }

    #[test]
    fn repeated_merges_remain_valid() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        let ab = dir.path().join("ab.pdf");
        let output = dir.path().join("again.pdf");

        write_test_pdf(
            &a,
            &[
                TestPage::new(595.0, 842.0, 0, "A1", None),
                TestPage::new(612.0, 792.0, 0, "A2", Some(0)),
            ],
        );
        write_test_pdf(&b, &[TestPage::new(595.0, 842.0, 0, "B1", None)]);

        merge_pdfs(&[a.clone(), b], &ab).unwrap();
        let outcome = merge_pdfs(&[ab.clone(), a], &output).unwrap();
        assert_eq!(outcome.pages, 5);

        let document = Document::load(&output).unwrap();
        assert_eq!(page_ids(&document).len(), 5);
        assert!(page_text(&document, page_ids(&document)[0]).contains("A1"));
    }

    #[test]
    fn rejects_invalid_references() {
        let dir = tempdir().unwrap();
        let valid = dir.path().join("valid.pdf");
        let broken = dir.path().join("broken.pdf");
        let output = dir.path().join("merged.pdf");
        write_test_pdf(&valid, &[TestPage::new(595.0, 842.0, 0, "Valid", None)]);

        let mut document = Document::load(&valid).unwrap();
        let page_id = *document.get_pages().values().next().unwrap();
        document
            .get_object_mut(page_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Contents", Object::Reference((9999, 0)));
        document.save(&broken).unwrap();

        assert!(matches!(
            merge_pdfs(&[broken, valid], &output),
            Err(MergeError::InvalidReference { .. })
        ));
        assert!(!output.exists());
    }

    #[test]
    fn rejects_malformed_and_encrypted_pdfs() {
        let dir = tempdir().unwrap();
        let valid = dir.path().join("valid.pdf");
        let other = dir.path().join("other.pdf");
        write_test_pdf(&valid, &[TestPage::new(595.0, 842.0, 0, "Valid", None)]);
        write_test_pdf(&other, &[TestPage::new(595.0, 842.0, 0, "Other", None)]);

        let truncated = dir.path().join("truncated.pdf");
        let bytes = fs::read(&valid).unwrap();
        fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            merge_pdfs(&[truncated, other.clone()], &dir.path().join("bad.pdf")),
            Err(MergeError::InvalidPdf { .. })
        ));

        let encrypted = dir.path().join("encrypted.pdf");
        write_encrypted_pdf(&valid, &encrypted);
        assert!(matches!(
            merge_pdfs(&[encrypted, other], &dir.path().join("encrypted-out.pdf")),
            Err(MergeError::EncryptedPdf { .. })
        ));
    }

    #[test]
    fn merges_large_documents() {
        let dir = tempdir().unwrap();
        let large = dir.path().join("large.pdf");
        let small = dir.path().join("small.pdf");
        let output = dir.path().join("merged.pdf");

        let large_pages: Vec<TestPage> = (0..300)
            .map(|index| {
                TestPage::new(595.0, 842.0, 0, format!("Large page {index}"), None)
            })
            .collect();
        write_test_pdf(&large, &large_pages);
        write_test_pdf(
            &small,
            &[TestPage::new(595.0, 842.0, 0, "Small page", None)],
        );

        let outcome = merge_pdfs(&[large, small], &output).unwrap();
        assert_eq!(outcome.pages, 301);

        let document = Document::load(&output).unwrap();
        assert_eq!(page_ids(&document).len(), 301);
    }

    #[test]
    fn cleanup_after_failure_leaves_no_output_or_temp_files() {
        let dir = tempdir().unwrap();
        let valid = dir.path().join("valid.pdf");
        let invalid = dir.path().join("invalid.pdf");
        let output = dir.path().join("failed.pdf");
        write_test_pdf(&valid, &[TestPage::new(595.0, 842.0, 0, "Valid", None)]);
        fs::write(&invalid, b"not a pdf").unwrap();

        assert!(merge_pdfs(&[valid, invalid], &output).is_err());
        assert!(!output.exists());

        let temp_files = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp"))
            .count();
        assert_eq!(temp_files, 0);
    }

    #[test]
    fn never_overwrites_unless_explicitly_allowed() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        let output = dir.path().join("merged.pdf");
        write_test_pdf(&a, &[TestPage::new(595.0, 842.0, 0, "A", None)]);
        write_test_pdf(&b, &[TestPage::new(595.0, 842.0, 0, "B", None)]);
        fs::write(&output, b"sentinel").unwrap();

        assert!(matches!(
            merge_pdfs(&[a.clone(), b.clone()], &output),
            Err(MergeError::OutputExists { .. })
        ));
        assert_eq!(fs::read(&output).unwrap(), b"sentinel");

        merge_pdfs_with_overwrite(&[a, b], &output).unwrap();
        assert!(fs::read(&output).unwrap().starts_with(b"%PDF"));
    }

    #[test]
    fn enforces_input_count_limits() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        write_test_pdf(&input, &[TestPage::new(595.0, 842.0, 0, "A", None)]);
        let output = dir.path().join("out.pdf");

        assert!(matches!(
            merge_pdfs(&[input.clone()], &output),
            Err(MergeError::TooFewInputs)
        ));

        let too_many = vec![input; MAX_MERGE_INPUTS + 1];
        assert!(matches!(
            merge_pdfs(&too_many, &output),
            Err(MergeError::TooManyInputs { max: MAX_MERGE_INPUTS })
        ));
    }

    #[test]
    fn reports_missing_output_directory() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        write_test_pdf(&a, &[TestPage::new(595.0, 842.0, 0, "A", None)]);
        write_test_pdf(&b, &[TestPage::new(595.0, 842.0, 0, "B", None)]);

        let output = dir.path().join("missing").join("merged.pdf");
        assert!(matches!(
            merge_pdfs(&[a, b], &output),
            Err(MergeError::OutputDirectoryInvalid { .. })
        ));
    }
}
