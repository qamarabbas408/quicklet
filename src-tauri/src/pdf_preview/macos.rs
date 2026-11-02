use std::path::Path;
use std::sync::{Mutex, OnceLock};

use objc2::AnyThread;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_foundation::{NSDataBase64EncodingOptions, NSDictionary, NSSize, NSString, NSURL};
use objc2_pdf_kit::{PDFDisplayBox, PDFDocument};

use super::{PreviewBatch, PreviewError, Thumbnail, ThumbnailRequest};

static PDFKIT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(super) fn render(
    path: &Path,
    requests: &[ThumbnailRequest],
    target_width: u32,
    target_height: u32,
) -> Result<PreviewBatch, PreviewError> {
    let lock = PDFKIT_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock
        .lock()
        .map_err(|_| PreviewError::RenderFailed("PDFKit renderer is unavailable.".to_string()))?;

    let path_str = path
        .to_str()
        .ok_or_else(|| PreviewError::InvalidRequest("PDF path is not valid UTF-8.".to_string()))?;

    autoreleasepool(|_| {
        let ns_path = NSString::from_str(path_str);
        let url = NSURL::fileURLWithPath(&ns_path);
        let document =
            unsafe { PDFDocument::initWithURL(PDFDocument::alloc(), &url) }.ok_or_else(|| {
                PreviewError::InvalidPdf {
                    reason: "PDFKit could not open the document.".to_string(),
                }
            })?;

        if unsafe { document.isEncrypted() } || unsafe { document.isLocked() } {
            return Err(PreviewError::EncryptedPdf);
        }

        let page_count = unsafe { document.pageCount() };
        if page_count == 0 {
            return Err(PreviewError::InvalidPdf {
                reason: "document has no pages".to_string(),
            });
        }

        let mut thumbnails = Vec::with_capacity(requests.len());
        for request in requests {
            if request.page > page_count {
                return Err(PreviewError::InvalidPage {
                    page: request.page,
                    max: page_count,
                });
            }

            let page = unsafe { document.pageAtIndex(request.page - 1) }.ok_or_else(|| {
                PreviewError::RenderFailed(format!("page {} is unavailable", request.page))
            })?;
            let raw_rotation = unsafe { page.rotation() } as i64;
            if raw_rotation % 90 != 0 {
                return Err(PreviewError::InvalidSourceRotation {
                    page: request.page,
                    value: raw_rotation,
                });
            }
            let source_rotation = raw_rotation.rem_euclid(360) as u16;
            let effective_rotation =
                (i64::from(source_rotation) + 90 * i64::from(request.turns)).rem_euclid(360) as u16;
            unsafe { page.setRotation(effective_rotation as isize) };

            let image = unsafe {
                page.thumbnailOfSize_forBox(
                    NSSize {
                        width: f64::from(target_width),
                        height: f64::from(target_height),
                    },
                    PDFDisplayBox::CropBox,
                )
            };
            let tiff = image
                .TIFFRepresentation()
                .ok_or_else(|| PreviewError::RenderFailed("TIFF encoding failed.".to_string()))?;
            let bitmap = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff)
                .ok_or_else(|| {
                    PreviewError::RenderFailed("bitmap conversion failed.".to_string())
                })?;
            let properties = NSDictionary::new();
            let png = unsafe {
                bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
            }
            .ok_or_else(|| PreviewError::RenderFailed("PNG encoding failed.".to_string()))?;
            let png_bytes = png.to_vec();
            let (width, height) = png_dimensions(&png_bytes)?;
            let png_base64 = png
                .base64EncodedStringWithOptions(NSDataBase64EncodingOptions(0))
                .to_string();

            thumbnails.push(Thumbnail {
                page: request.page,
                source_rotation,
                effective_rotation,
                width,
                height,
                png_base64,
            });
        }

        Ok(PreviewBatch {
            page_count,
            thumbnails,
        })
    })
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), PreviewError> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(PreviewError::RenderFailed(
            "PDFKit returned invalid PNG data.".to_string(),
        ));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;

    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions};
    use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    struct TestPage {
        width: f32,
        height: f32,
        rotation: i32,
        text: &'static str,
    }

    fn write_fixture(path: &Path) {
        let pages = [
            TestPage {
                width: 612.0,
                height: 792.0,
                rotation: 0,
                text: "A1",
            },
            TestPage {
                width: 400.0,
                height: 300.0,
                rotation: 90,
                text: "B2",
            },
            TestPage {
                width: 700.0,
                height: 500.0,
                rotation: 180,
                text: "C3",
            },
            TestPage {
                width: 300.0,
                height: 600.0,
                rotation: 270,
                text: "D4",
            },
        ];
        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let font = Ref::new(3);
        let page_refs: Vec<Ref> = (0..pages.len())
            .map(|i| Ref::new(4 + i as i32 * 2))
            .collect();
        let content_refs: Vec<Ref> = (0..pages.len())
            .map(|i| Ref::new(5 + i as i32 * 2))
            .collect();

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree)
            .kids(page_refs.iter().copied())
            .count(pages.len() as i32);
        for (index, page) in pages.iter().enumerate() {
            let mut writer = pdf.page(page_refs[index]);
            writer.media_box(Rect::new(0.0, 0.0, page.width, page.height));
            writer.parent(tree);
            writer.contents(content_refs[index]);
            if page.rotation != 0 {
                writer.rotate(page.rotation);
            }
            writer.resources().fonts().pair(Name(b"F1"), font);
        }
        pdf.type1_font(font).base_font(Name(b"Helvetica"));
        for (index, content_ref) in content_refs.iter().enumerate() {
            let mut content = Content::new();
            content.begin_text();
            content.set_font(Name(b"F1"), 20.0);
            content.next_line(72.0, 200.0);
            content.show(Str(pages[index].text.as_bytes()));
            content.end_text();
            pdf.stream(*content_ref, &content.finish());
        }
        fs::write(path, pdf.finish()).unwrap();
    }

    fn write_encrypted_pdf(source: &Path, output: &Path) {
        let mut document = Document::load(source).unwrap();
        let file_id = Object::string_literal(b"quicklet-preview-test");
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

    fn png_prefix(base64: &str) -> bool {
        base64.starts_with("iVBORw0KGgo")
    }

    #[test]
    fn renders_pages_rotations_and_metadata() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("fixture.pdf");
        write_fixture(&source);

        let result = super::super::render_thumbnails(
            &source,
            &[
                ThumbnailRequest { page: 1, turns: 0 },
                ThumbnailRequest { page: 2, turns: 0 },
                ThumbnailRequest { page: 3, turns: 0 },
                ThumbnailRequest { page: 4, turns: 0 },
            ],
            240,
            320,
        )
        .unwrap();
        assert_eq!(result.page_count, 4);
        assert_eq!(result.thumbnails.len(), 4);
        assert_eq!(
            result
                .thumbnails
                .iter()
                .map(|thumb| thumb.effective_rotation)
                .collect::<Vec<_>>(),
            vec![0, 90, 180, 270]
        );
        assert_eq!(
            result
                .thumbnails
                .iter()
                .map(|thumb| (thumb.width, thumb.height))
                .collect::<Vec<_>>(),
            vec![(240, 310), (240, 320), (240, 171), (240, 120)]
        );
        assert!(
            result
                .thumbnails
                .iter()
                .all(|thumb| png_prefix(&thumb.png_base64))
        );

        let rotated = super::super::render_thumbnails(
            &source,
            &[
                ThumbnailRequest { page: 1, turns: 1 },
                ThumbnailRequest { page: 1, turns: 1 },
            ],
            240,
            320,
        );
        assert!(matches!(rotated, Err(PreviewError::InvalidRequest(_))));

        let rotated = super::super::render_thumbnails(
            &source,
            &[ThumbnailRequest { page: 1, turns: 1 }],
            240,
            320,
        )
        .unwrap()
        .thumbnails
        .remove(0);
        assert_eq!(rotated.source_rotation, 0);
        assert_eq!(rotated.effective_rotation, 90);
        assert_eq!((rotated.width, rotated.height), (240, 185));
    }

    #[test]
    fn rejects_bad_documents_and_indices() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("fixture.pdf");
        write_fixture(&source);

        let out_of_range = super::super::render_thumbnails(
            &source,
            &[ThumbnailRequest { page: 5, turns: 0 }],
            240,
            320,
        );
        assert!(matches!(
            out_of_range,
            Err(PreviewError::InvalidPage { page: 5, max: 4 })
        ));

        let encrypted = dir.path().join("encrypted.pdf");
        write_encrypted_pdf(&source, &encrypted);
        assert!(matches!(
            super::super::render_thumbnails(
                &encrypted,
                &[ThumbnailRequest { page: 1, turns: 0 }],
                240,
                320
            ),
            Err(PreviewError::EncryptedPdf)
        ));

        let bytes = fs::read(&source).unwrap();
        let malformed = dir.path().join("malformed.pdf");
        fs::write(&malformed, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            super::super::render_thumbnails(
                &malformed,
                &[ThumbnailRequest { page: 1, turns: 0 }],
                240,
                320
            ),
            Err(PreviewError::InvalidPdf { .. }) | Err(PreviewError::RenderFailed(_))
        ));

        let missing = dir.path().join("missing.pdf");
        assert!(matches!(
            super::super::render_thumbnails(
                &missing,
                &[ThumbnailRequest { page: 1, turns: 0 }],
                240,
                320
            ),
            Err(PreviewError::InputNotFound { .. })
        ));
    }

    #[test]
    fn supports_repeated_sessions_and_serialized_threads() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("fixture.pdf");
        write_fixture(&source);

        for _ in 0..20 {
            let result = super::super::render_thumbnails(
                &source,
                &[ThumbnailRequest { page: 1, turns: 0 }],
                120,
                160,
            )
            .unwrap();
            assert_eq!(result.thumbnails.len(), 1);
        }

        let handles = (0..2)
            .map(|_| {
                let source = source.clone();
                thread::spawn(move || {
                    for _ in 0..5 {
                        let result = super::super::render_thumbnails(
                            &source,
                            &[ThumbnailRequest { page: 2, turns: 1 }],
                            120,
                            160,
                        )
                        .unwrap();
                        assert_eq!(result.thumbnails[0].effective_rotation, 180);
                    }
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap();
        }
    }
}
