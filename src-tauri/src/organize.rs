//! Extract/Delete Pages planning and single-output writing.
//!
//! Page copying, link filtering, and safe persistence are shared with the
//! Split PDF engine. Catalog-level features that Split PDF does not preserve
//! are also not preserved here: bookmarks/outlines, named destinations,
//! AcroForm/XFA fields, tagged-PDF structure, optional content groups, page
//! labels, document metadata, and digital signatures. Encrypted PDFs are
//! rejected. Direct links are kept when both ends remain in the output.

use std::collections::HashSet;
use std::fmt;

use crate::split::{self, SplitError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PageSelectionMode {
    Extract,
    Delete,
}

impl PageSelectionMode {
    pub fn from_action_id(action_id: &str) -> Option<Self> {
        match action_id {
            "pdf.extract_pages" => Some(Self::Extract),
            "pdf.delete_pages" => Some(Self::Delete),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Extract => "Extract",
            Self::Delete => "Delete",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSelectionPlan {
    pub mode: PageSelectionMode,
    pub page_count: usize,
    /// Extract: selected pages in the requested order.
    /// Delete: pages to remove in the requested range order.
    pub selected_pages: Vec<usize>,
    /// Extract: selected pages in the requested order.
    /// Delete: remaining pages in original document order.
    pub output_pages: Vec<usize>,
    pub output_count: usize,
}

#[derive(Clone, Debug)]
pub struct PageSelectionOutcome {
    pub pages: usize,
    pub bytes_written: u64,
}

#[derive(Debug)]
pub enum OrganizeError {
    InvalidSelection(String),
    NoPages,
    Processing(String),
}

impl fmt::Display for OrganizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSelection(message) => formatter.write_str(message),
            Self::NoPages => write!(
                formatter,
                "That selection would remove every page. Keep at least one page."
            ),
            Self::Processing(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for OrganizeError {}

impl From<SplitError> for OrganizeError {
    fn from(error: SplitError) -> Self {
        match &error {
            SplitError::InvalidRanges(_) | SplitError::TooManyRanges { .. } => {
                Self::InvalidSelection(error.to_string())
            }
            _ => Self::Processing(error.to_string()),
        }
    }
}

pub fn plan(
    page_count: usize,
    selection: &str,
    mode: PageSelectionMode,
) -> Result<PageSelectionPlan, OrganizeError> {
    if page_count == 0 {
        return Err(OrganizeError::NoPages);
    }
    if page_count > split::MAX_SPLIT_PAGES {
        return Err(OrganizeError::InvalidSelection(format!(
            "This PDF has too many pages. The limit is {} pages.",
            split::MAX_SPLIT_PAGES
        )));
    }

    let ranges = split::parse_page_ranges(selection, page_count)?;
    let selected_pages = ranges
        .iter()
        .flat_map(|range| range.start..=range.end)
        .collect::<Vec<_>>();

    let output_pages = match mode {
        PageSelectionMode::Extract => selected_pages.clone(),
        PageSelectionMode::Delete => {
            let removed: HashSet<usize> = selected_pages.iter().copied().collect();
            let remaining = (1..=page_count)
                .filter(|page| !removed.contains(page))
                .collect::<Vec<_>>();
            if remaining.is_empty() {
                return Err(OrganizeError::NoPages);
            }
            remaining
        }
    };

    Ok(PageSelectionPlan {
        mode,
        page_count,
        selected_pages,
        output_count: output_pages.len(),
        output_pages,
    })
}

pub fn extract(
    input: &std::path::Path,
    output: &std::path::Path,
    selection: &str,
    allow_overwrite: bool,
) -> Result<PageSelectionOutcome, OrganizeError> {
    write(
        input,
        output,
        selection,
        PageSelectionMode::Extract,
        allow_overwrite,
    )
}

pub fn delete(
    input: &std::path::Path,
    output: &std::path::Path,
    selection: &str,
    allow_overwrite: bool,
) -> Result<PageSelectionOutcome, OrganizeError> {
    write(
        input,
        output,
        selection,
        PageSelectionMode::Delete,
        allow_overwrite,
    )
}

fn write(
    input: &std::path::Path,
    output: &std::path::Path,
    selection: &str,
    mode: PageSelectionMode,
    allow_overwrite: bool,
) -> Result<PageSelectionOutcome, OrganizeError> {
    let source = split::inspect_pdf(input)?;
    let plan = plan(source.pages, selection, mode)?;
    let outcome = split::write_page_selection(input, output, &plan.output_pages, allow_overwrite)?;

    Ok(PageSelectionOutcome {
        pages: outcome.pages,
        bytes_written: outcome.bytes_written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::{
        Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions,
    };
    use pdf_writer::types::{ActionType, AnnotationType};
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};
    use std::collections::BTreeMap;
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
        let info = Ref::new(next_id);
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
            writer.media_box(Rect::new(0.0, 0.0, page.width, page.height));
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
        pdf.document_info(info)
            .title(pdf_writer::TextStr("Page organizer fixture"))
            .producer(pdf_writer::TextStr("Quicklet tests"));

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
                annotation.rect(Rect::new(72.0, 650.0, 280.0, 680.0));
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

    fn page_ids(document: &Document) -> Vec<ObjectId> {
        document.get_pages().into_values().collect()
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

    fn rotation(document: &Document, id: ObjectId) -> i64 {
        page_dictionary(document, id)
            .get(b"Rotate")
            .and_then(Object::as_i64)
            .unwrap_or(0)
    }

    fn page_text(document: &Document, id: ObjectId) -> String {
        String::from_utf8_lossy(&document.get_page_content(id)).into_owned()
    }

    fn link_targets(document: &Document, id: ObjectId) -> Vec<u32> {
        let page_numbers: HashMap<ObjectId, u32> = document
            .get_pages()
            .iter()
            .map(|(number, id)| (*id, *number))
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

    fn fixture(path: &Path) {
        write_test_pdf(
            path,
            &[
                TestPage::new(500.0, 700.0, 0, "Page 1", Some(2)),
                TestPage::new(510.0, 710.0, 90, "Page 2", None),
                TestPage::new(520.0, 720.0, 180, "Page 3", None),
                TestPage::new(530.0, 730.0, 0, "Page 4", None),
                TestPage::new(540.0, 740.0, 90, "Page 5", None),
                TestPage::new(550.0, 750.0, 0, "Page 6", None),
            ],
        );
    }

    fn write_encrypted_pdf(source: &Path, output: &Path) {
        let mut document = Document::load(source).unwrap();
        let file_id = Object::string_literal(b"quicklet-organize-test");
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

    #[test]
    fn extracts_in_requested_order_with_links_and_page_properties() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        let output = dir.path().join("extract.pdf");
        fixture(&input);

        let plan = plan(6, "3, 1-2, 5", PageSelectionMode::Extract).unwrap();
        assert_eq!(plan.output_pages, vec![3, 1, 2, 5]);
        assert_eq!(plan.output_count, 4);
        assert_eq!(plan.selected_pages, vec![3, 1, 2, 5]);

        let outcome = extract(&input, &output, "3, 1-2, 5", true).unwrap();
        assert_eq!(outcome.pages, 4);
        let document = Document::load(&output).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 4);
        assert!(page_text(&document, ids[0]).contains("Page 3"));
        assert!(page_text(&document, ids[1]).contains("Page 1"));
        assert!(page_text(&document, ids[2]).contains("Page 2"));
        assert!(page_text(&document, ids[3]).contains("Page 5"));
        assert_eq!(media_box(&document, ids[0]), (520.0, 720.0));
        assert_eq!(rotation(&document, ids[0]), 180);
        assert_eq!(media_box(&document, ids[2]), (510.0, 710.0));
        assert_eq!(rotation(&document, ids[2]), 90);
        assert_eq!(link_targets(&document, ids[1]), vec![1]);
    }

    #[test]
    fn deletes_selected_pages_and_keeps_original_order() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        let output = dir.path().join("delete.pdf");
        fixture(&input);

        let plan = plan(6, "2, 5-6", PageSelectionMode::Delete).unwrap();
        assert_eq!(plan.selected_pages, vec![2, 5, 6]);
        assert_eq!(plan.output_pages, vec![1, 3, 4]);
        assert_eq!(plan.output_count, 3);

        let outcome = delete(&input, &output, "2, 5-6", true).unwrap();
        assert_eq!(outcome.pages, 3);
        let document = Document::load(&output).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 3);
        assert!(page_text(&document, ids[0]).contains("Page 1"));
        assert!(page_text(&document, ids[1]).contains("Page 3"));
        assert!(page_text(&document, ids[2]).contains("Page 4"));
        assert_eq!(link_targets(&document, ids[0]), vec![2]);
    }

    #[test]
    fn excluded_link_targets_are_dropped() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        let extract_out = dir.path().join("extract.pdf");
        let delete_out = dir.path().join("delete.pdf");
        fixture(&input);

        extract(&input, &extract_out, "1", true).unwrap();
        let document = Document::load(&extract_out).unwrap();
        let ids = page_ids(&document);
        assert!(link_targets(&document, ids[0]).is_empty());

        delete(&input, &delete_out, "3", true).unwrap();
        let document = Document::load(&delete_out).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 5);
        assert!(link_targets(&document, ids[0]).is_empty());
    }

    #[test]
    fn validates_selections_and_rejects_deleting_every_page() {
        for selection in ["", "0", "3-1", "1-2, 2-3", "1-2, 1", "1,,2", "a", "7"] {
            assert!(
                plan(6, selection, PageSelectionMode::Extract).is_err(),
                "extract accepted {selection:?}"
            );
            assert!(
                plan(6, selection, PageSelectionMode::Delete).is_err(),
                "delete accepted {selection:?}"
            );
        }

        let error = plan(6, "1-6", PageSelectionMode::Delete).unwrap_err();
        assert!(matches!(error, OrganizeError::NoPages));
        assert!(plan(6, "1-6", PageSelectionMode::Extract).is_ok());

        let unordered = plan(6, "3, 1-2, 5", PageSelectionMode::Extract).unwrap();
        assert_eq!(unordered.output_pages, vec![3, 1, 2, 5]);

        let single = plan(6, "6", PageSelectionMode::Extract).unwrap();
        assert_eq!(single.output_count, 1);
        assert_eq!(single.output_pages, vec![6]);
    }

    #[test]
    fn rejects_same_path_and_existing_destination_without_overwrite() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        let output = dir.path().join("output.pdf");
        fixture(&input);
        let original = fs::read(&input).unwrap();

        assert!(matches!(
            extract(&input, &input, "1", true),
            Err(OrganizeError::Processing(_))
        ));
        assert_eq!(fs::read(&input).unwrap(), original);

        fs::write(&output, b"keep me").unwrap();
        assert!(matches!(
            extract(&input, &output, "1", false),
            Err(OrganizeError::Processing(_))
        ));
        assert_eq!(fs::read(&output).unwrap(), b"keep me");
        assert_eq!(fs::read(&input).unwrap(), original);

        extract(&input, &output, "1", true).unwrap();
        assert_ne!(fs::read(&output).unwrap(), b"keep me");
        assert_eq!(fs::read(&input).unwrap(), original);
    }

    #[test]
    fn rejects_malformed_and_encrypted_inputs() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("broken.pdf");
        let output = dir.path().join("output.pdf");
        fs::write(&input, b"not a pdf").unwrap();

        assert!(extract(&input, &output, "1", true).is_err());
        assert!(!output.exists());

        let source = dir.path().join("source.pdf");
        let encrypted = dir.path().join("encrypted.pdf");
        fixture(&source);
        write_encrypted_pdf(&source, &encrypted);
        assert!(extract(&encrypted, &output, "1", true).is_err());
        assert!(delete(&encrypted, &output, "2", true).is_err());
        assert!(!output.exists());
    }
}
