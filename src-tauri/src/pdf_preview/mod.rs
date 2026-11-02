//! Bounded macOS PDFKit thumbnail bridge.
//!
//! The frontend never supplies a filesystem path. The Tauri command resolves
//! the current PDF from Rust-owned input state and passes only page requests,
//! quarter-turn overrides, and a target thumbnail box to this module.
//!
//! PDFKit objects are created, used, and dropped on the blocking worker thread
//! selected by the Tauri command. Native calls are serialized. Each batch uses
//! an autorelease pool, and the request size/page count/batch count are bounded
//! before any PDFKit work starts.

use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::rotate::MAX_ROTATE_INPUT_BYTES;

pub const MAX_THUMBNAIL_BATCH: usize = 12;
pub const MIN_THUMBNAIL_WIDTH: u32 = 64;
pub const MAX_THUMBNAIL_WIDTH: u32 = 480;
pub const MIN_THUMBNAIL_HEIGHT: u32 = 64;
pub const MAX_THUMBNAIL_HEIGHT: u32 = 640;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailRequest {
    /// 1-based page number.
    pub page: usize,
    /// Additional clockwise quarter turns for preview only.
    pub turns: u8,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub page: usize,
    pub source_rotation: u16,
    pub effective_rotation: u16,
    pub width: u32,
    pub height: u32,
    /// PNG bytes as base64. The frontend can use a `data:image/png;base64,...` URL.
    pub png_base64: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewBatch {
    pub page_count: usize,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug)]
pub enum PreviewError {
    InputNotFound { path: std::path::PathBuf },
    InputTooLarge { bytes: u64, max: u64 },
    InvalidPdf { reason: String },
    EncryptedPdf,
    InvalidRequest(String),
    InvalidPage { page: usize, max: usize },
    InvalidSourceRotation { page: usize, value: i64 },
    RenderFailed(String),
    UnsupportedPlatform,
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputNotFound { path } => {
                write!(formatter, "PDF not found: {}", path.display())
            }
            Self::InputTooLarge { bytes, max } => write!(
                formatter,
                "This PDF is too large for preview ({:.0} MB). The limit is {:.0} MB.",
                *bytes as f64 / (1024.0 * 1024.0),
                *max as f64 / (1024.0 * 1024.0)
            ),
            Self::InvalidPdf { reason } => {
                write!(formatter, "Could not preview this PDF: {reason}")
            }
            Self::EncryptedPdf => write!(
                formatter,
                "Encrypted PDFs are not supported. Remove the password and try again."
            ),
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::InvalidPage { page, max } => write!(
                formatter,
                "Page {page} is outside this PDF. It has {max} {}.",
                if *max == 1 { "page" } else { "pages" }
            ),
            Self::InvalidSourceRotation { page, value } => write!(
                formatter,
                "Page {page} has an unsupported /Rotate value ({value}). Rotation must be a multiple of 90."
            ),
            Self::RenderFailed(message) => write!(formatter, "Could not render preview: {message}"),
            Self::UnsupportedPlatform => {
                write!(
                    formatter,
                    "PDF preview is not available on this platform yet."
                )
            }
        }
    }
}

impl std::error::Error for PreviewError {}

pub fn render_thumbnails(
    path: &Path,
    requests: &[ThumbnailRequest],
    target_width: u32,
    target_height: u32,
) -> Result<PreviewBatch, PreviewError> {
    validate_request(path, requests, target_width, target_height)?;

    #[cfg(target_os = "macos")]
    {
        macos::render(path, requests, target_width, target_height)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        let _ = requests;
        Err(PreviewError::UnsupportedPlatform)
    }
}

fn validate_request(
    path: &Path,
    requests: &[ThumbnailRequest],
    target_width: u32,
    target_height: u32,
) -> Result<(), PreviewError> {
    if requests.is_empty() {
        return Err(PreviewError::InvalidRequest(
            "Request at least one page thumbnail.".to_string(),
        ));
    }
    if requests.len() > MAX_THUMBNAIL_BATCH {
        return Err(PreviewError::InvalidRequest(format!(
            "Request at most {MAX_THUMBNAIL_BATCH} thumbnails at a time."
        )));
    }
    if !(MIN_THUMBNAIL_WIDTH..=MAX_THUMBNAIL_WIDTH).contains(&target_width)
        || !(MIN_THUMBNAIL_HEIGHT..=MAX_THUMBNAIL_HEIGHT).contains(&target_height)
    {
        return Err(PreviewError::InvalidRequest(format!(
            "Thumbnail size must be between {}x{} and {}x{}.",
            MIN_THUMBNAIL_WIDTH, MIN_THUMBNAIL_HEIGHT, MAX_THUMBNAIL_WIDTH, MAX_THUMBNAIL_HEIGHT
        )));
    }

    let mut seen = std::collections::HashSet::new();
    for request in requests {
        if request.page == 0 {
            return Err(PreviewError::InvalidPage { page: 0, max: 0 });
        }
        if request.turns > 3 {
            return Err(PreviewError::InvalidRequest(format!(
                "Page {} has an invalid preview rotation step ({}).",
                request.page, request.turns
            )));
        }
        if !seen.insert(request.page) {
            return Err(PreviewError::InvalidRequest(format!(
                "Page {} was requested more than once.",
                request.page
            )));
        }
    }

    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(PreviewError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(error) => {
            return Err(PreviewError::RenderFailed(format!(
                "Could not read the PDF: {error}"
            )));
        }
    };
    if metadata.is_dir() {
        return Err(PreviewError::InvalidPdf {
            reason: "path is a folder".to_string(),
        });
    }
    if metadata.len() > MAX_ROTATE_INPUT_BYTES {
        return Err(PreviewError::InputTooLarge {
            bytes: metadata.len(),
            max: MAX_ROTATE_INPUT_BYTES,
        });
    }

    Ok(())
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_batches_before_native_rendering() {
        let path = std::env::current_exe().unwrap();
        let invalid = [
            (vec![], 240, 320),
            (
                vec![ThumbnailRequest { page: 1, turns: 0 }; MAX_THUMBNAIL_BATCH + 1],
                240,
                320,
            ),
            (vec![ThumbnailRequest { page: 0, turns: 0 }], 240, 320),
            (
                vec![
                    ThumbnailRequest { page: 1, turns: 0 },
                    ThumbnailRequest { page: 1, turns: 2 },
                ],
                240,
                320,
            ),
            (vec![ThumbnailRequest { page: 1, turns: 4 }], 240, 320),
            (vec![ThumbnailRequest { page: 1, turns: 0 }], 32, 320),
            (vec![ThumbnailRequest { page: 1, turns: 0 }], 240, 9999),
        ];

        for (requests, width, height) in invalid {
            let result = render_thumbnails(&path, &requests, width, height);
            assert!(
                matches!(
                    result,
                    Err(PreviewError::InvalidRequest(_)) | Err(PreviewError::InvalidPage { .. })
                ),
                "unexpected result: {result:?}"
            );
        }
    }

    #[test]
    fn reports_missing_file() {
        let result = render_thumbnails(
            Path::new("/tmp/quicklet-preview-definitely-missing.pdf"),
            &[ThumbnailRequest { page: 1, turns: 0 }],
            240,
            320,
        );
        assert!(matches!(result, Err(PreviewError::InputNotFound { .. })));
    }
}
