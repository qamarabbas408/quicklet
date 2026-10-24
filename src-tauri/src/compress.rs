//! Compresses PDF streams and compatible images.
//!
//! Supported: Flate compression of uncompressed non-font streams, object/xref
//! streams, and lossy JPEG recompression of 8-bit DeviceRGB/DeviceGray JPEG or
//! Flate images. Page order, geometry, rotation, text, fonts, vector content,
//! annotations, links, transparency masks, metadata, and document structures
//! are retained as objects.
//!
//! Not recompressed: SMask/stencil/color masks, image masks, ICCBased/CMYK/
//! Indexed images, non-8-bit images, JPX/CCITT/JBIG2 images, and streams with
//! `/Decode`. Image candidates are only used when they save at least 5% and
//! 1 KiB. If the final output is not smaller, the exact original bytes are
//! retained instead.
//!
//! Encrypted PDFs are rejected. Digital signatures become invalid after any
//! rewrite. Linearization is not preserved and the saved PDF version becomes
//! at least 1.5 when object/xref streams are used.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{
    DynamicImage, ExtendedColorType, GrayImage, ImageFormat, ImageReader, Limits, RgbImage,
};
use lopdf::{Dictionary, Document, Object, ObjectId, SaveOptions, Stream};
use tempfile::NamedTempFile;

pub const MAX_COMPRESS_INPUT_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_COMPRESS_PAGES: usize = 2_000;
pub const MAX_COMPRESS_IMAGE_OBJECTS: usize = 500;
pub const MAX_DECODED_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const MIN_IMAGE_SAVING_BYTES: u64 = 1024;
const MIN_IMAGE_SAVING_PERCENT: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompressionPreset {
    Light,
    Balanced,
    Strong,
}

impl CompressionPreset {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "balanced" => Some(Self::Balanced),
            "strong" => Some(Self::Strong),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Balanced => "Balanced",
            Self::Strong => "Strong",
        }
    }

    fn image_quality(self) -> Option<u8> {
        match self {
            Self::Light => None,
            Self::Balanced => Some(75),
            Self::Strong => Some(50),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompressionStatus {
    Reduced,
    NoReduction,
}

#[derive(Clone, Debug)]
pub struct CompressSource {
    pub pages: usize,
    pub bytes: u64,
    pub has_signature: bool,
    pub images: usize,
}

#[derive(Clone, Debug)]
pub struct CompressOutcome {
    pub pages: usize,
    pub original_bytes: u64,
    pub output_bytes: u64,
    pub saved_bytes: u64,
    pub reduction_percent: f64,
    pub status: CompressionStatus,
    pub images_seen: usize,
    pub images_recompressed: usize,
    pub images_skipped: usize,
}

#[derive(Debug)]
pub enum CompressError {
    InputNotFound { path: PathBuf },
    InputRead { path: PathBuf, source: io::Error },
    InputTooLarge { path: PathBuf, bytes: u64, max: u64 },
    InvalidPdf { path: PathBuf, reason: String },
    EncryptedPdf { path: PathBuf },
    InvalidReference { path: PathBuf, reference: String },
    TooManyPages { max: usize },
    TooManyImages { max: usize },
    SameAsInput { path: PathBuf },
    OutputExists { path: PathBuf },
    OutputIsDirectory { path: PathBuf },
    OutputDirectoryInvalid { path: PathBuf },
    OutputWrite { path: PathBuf, source: io::Error },
    OutputValidation { path: PathBuf, reason: String },
}

impl fmt::Display for CompressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputNotFound { path } => write!(formatter, "PDF not found: {}", path.display()),
            Self::InputRead { path, source } => {
                write!(formatter, "Could not read {}: {source}", path.display())
            }
            Self::InputTooLarge { path, bytes, max } => write!(
                formatter,
                "{} is too large to compress ({:.0} MB). The limit is {:.0} MB.",
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
                "This PDF has too many pages to compress. The limit is {max} pages."
            ),
            Self::TooManyImages { max } => write!(
                formatter,
                "This PDF has too many image objects to compress. The limit is {max}."
            ),
            Self::SameAsInput { path } => write!(
                formatter,
                "The output must not overwrite the source PDF: {}",
                path.display()
            ),
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
            Self::OutputIsDirectory { path } => {
                write!(formatter, "The output path is a folder: {}", path.display())
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
            Self::OutputValidation { path, reason } => write!(
                formatter,
                "The generated PDF could not be validated: {}: {reason}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for CompressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InputRead { source, .. } | Self::OutputWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

struct LoadedDocument {
    document: Document,
    original_bytes: Vec<u8>,
    pages: usize,
    has_signature: bool,
    images: usize,
}

#[derive(Default)]
struct ImageStats {
    seen: usize,
    recompressed: usize,
    skipped: usize,
}

pub fn inspect_pdf(input: &Path) -> Result<CompressSource, CompressError> {
    let loaded = load_document(input)?;
    Ok(CompressSource {
        pages: loaded.pages,
        bytes: loaded.original_bytes.len() as u64,
        has_signature: loaded.has_signature,
        images: loaded.images,
    })
}

/// Compresses a PDF without overwriting an existing destination.
pub fn compress_pdf(
    input: &Path,
    output: &Path,
    preset: CompressionPreset,
) -> Result<CompressOutcome, CompressError> {
    compress(input, output, preset, false)
}

/// Like [`compress_pdf`], but atomically replaces an existing destination.
///
/// Use only after explicit confirmation in a native save dialog.
pub fn compress_pdf_with_overwrite(
    input: &Path,
    output: &Path,
    preset: CompressionPreset,
) -> Result<CompressOutcome, CompressError> {
    compress(input, output, preset, true)
}

fn compress(
    input: &Path,
    output: &Path,
    preset: CompressionPreset,
    allow_overwrite: bool,
) -> Result<CompressOutcome, CompressError> {
    let mut loaded = load_document(input)?;
    validate_output(input, output, allow_overwrite)?;

    let original_bytes = loaded.original_bytes;
    let original_size = original_bytes.len() as u64;
    let mut image_stats = ImageStats::default();

    let font_programs = font_program_ids(&loaded.document);
    compress_streams(&mut loaded.document, &font_programs);
    if let Some(quality) = preset.image_quality() {
        image_stats = recompress_images(&mut loaded.document, quality);
    }

    let mut candidate = save_modern(&mut loaded.document, input)?;
    let status = if candidate.len() as u64 >= original_size {
        candidate = original_bytes;
        CompressionStatus::NoReduction
    } else {
        CompressionStatus::Reduced
    };

    write_safely(output, &candidate, allow_overwrite, loaded.pages)?;

    let output_size = candidate.len() as u64;
    let saved_bytes = if status == CompressionStatus::NoReduction {
        0
    } else {
        original_size.saturating_sub(output_size)
    };
    let reduction_percent = if original_size == 0 || saved_bytes == 0 {
        0.0
    } else {
        saved_bytes as f64 / original_size as f64 * 100.0
    };

    Ok(CompressOutcome {
        pages: loaded.pages,
        original_bytes: original_size,
        output_bytes: output_size,
        saved_bytes,
        reduction_percent,
        status,
        images_seen: image_stats.seen,
        images_recompressed: image_stats.recompressed,
        images_skipped: image_stats.skipped,
    })
}

fn load_document(path: &Path) -> Result<LoadedDocument, CompressError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CompressError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(CompressError::InputRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if metadata.len() > MAX_COMPRESS_INPUT_BYTES {
        return Err(CompressError::InputTooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            max: MAX_COMPRESS_INPUT_BYTES,
        });
    }
    if metadata.is_dir() {
        return Err(CompressError::InvalidPdf {
            path: path.to_path_buf(),
            reason: "path is a directory".to_string(),
        });
    }

    let original_bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CompressError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(CompressError::InputRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let document =
        Document::load_mem(&original_bytes).map_err(|error| CompressError::InvalidPdf {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;

    if document.is_encrypted() {
        return Err(CompressError::EncryptedPdf {
            path: path.to_path_buf(),
        });
    }

    validate_references(&document, path)?;

    let pages = document.get_pages().len();
    if pages == 0 {
        return Err(CompressError::InvalidPdf {
            path: path.to_path_buf(),
            reason: "document has no pages".to_string(),
        });
    }
    if pages > MAX_COMPRESS_PAGES {
        return Err(CompressError::TooManyPages {
            max: MAX_COMPRESS_PAGES,
        });
    }

    let images = document
        .objects
        .values()
        .filter(|object| is_image_stream(object))
        .count();
    if images > MAX_COMPRESS_IMAGE_OBJECTS {
        return Err(CompressError::TooManyImages {
            max: MAX_COMPRESS_IMAGE_OBJECTS,
        });
    }

    let has_signature = has_digital_signature(&document);

    Ok(LoadedDocument {
        document,
        original_bytes,
        pages,
        has_signature,
        images,
    })
}

fn validate_references(document: &Document, source: &Path) -> Result<(), CompressError> {
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
                        .map_err(|_| CompressError::InvalidReference {
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

fn validate_output(
    input: &Path,
    output: &Path,
    allow_overwrite: bool,
) -> Result<(), CompressError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    if !parent.is_dir() {
        return Err(CompressError::OutputDirectoryInvalid {
            path: parent.to_path_buf(),
        });
    }

    let parent = fs::canonicalize(parent).map_err(|_| CompressError::OutputDirectoryInvalid {
        path: parent.to_path_buf(),
    })?;
    let file_name = output
        .file_name()
        .ok_or_else(|| CompressError::OutputDirectoryInvalid {
            path: parent.clone(),
        })?;
    let candidate = parent.join(file_name);
    let input_canonical = fs::canonicalize(input).map_err(|_| CompressError::InvalidPdf {
        path: input.to_path_buf(),
        reason: "could not resolve the source path".to_string(),
    })?;

    let output_canonical = if output.exists() {
        fs::canonicalize(output).map_err(|_| CompressError::OutputDirectoryInvalid {
            path: output.to_path_buf(),
        })?
    } else {
        candidate
    };

    if output_canonical == input_canonical {
        return Err(CompressError::SameAsInput {
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
            return Err(CompressError::SameAsInput {
                path: output.to_path_buf(),
            });
        }
    }

    if output.exists() {
        if output.is_dir() {
            return Err(CompressError::OutputIsDirectory {
                path: output.to_path_buf(),
            });
        }
        if !allow_overwrite {
            return Err(CompressError::OutputExists {
                path: output.to_path_buf(),
            });
        }
    }

    Ok(())
}

fn compress_streams(document: &mut Document, font_programs: &HashSet<ObjectId>) {
    for (id, object) in document.objects.iter_mut() {
        let Object::Stream(stream) = object else {
            continue;
        };
        if !stream.allows_compression
            || stream.dict.get(b"Filter").is_ok()
            || font_programs.contains(id)
            || is_font_program(&stream.dict)
        {
            continue;
        }

        let _ = stream.compress();
    }
}

fn font_program_ids(document: &Document) -> HashSet<ObjectId> {
    let mut ids = HashSet::new();
    for object in document.objects.values() {
        let Ok(dictionary) = object.as_dict() else {
            continue;
        };
        for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
            if let Ok(id) = dictionary.get(key).and_then(Object::as_reference) {
                ids.insert(id);
            }
        }
    }
    ids
}

fn is_font_program(dictionary: &Dictionary) -> bool {
    dictionary.get(b"Length1").is_ok()
        || dictionary.get(b"Length2").is_ok()
        || dictionary.get(b"Length3").is_ok()
}

fn recompress_images(document: &mut Document, quality: u8) -> ImageStats {
    let mut stats = ImageStats::default();
    let masks = mask_object_ids(document);

    for (id, object) in document.objects.iter_mut() {
        if !is_image_stream(object) {
            continue;
        }
        let Object::Stream(stream) = object else {
            continue;
        };

        stats.seen += 1;

        if masks.contains(id) {
            stats.skipped += 1;
            continue;
        }

        let Some(candidate) = encode_smaller_jpeg(stream, quality) else {
            stats.skipped += 1;
            continue;
        };

        let original_size = stream.content.len() as u64;
        let saving = original_size.saturating_sub(candidate.len() as u64);
        if candidate.len() >= stream.content.len()
            || saving < MIN_IMAGE_SAVING_BYTES
            || saving * 100 < original_size * MIN_IMAGE_SAVING_PERCENT
        {
            stats.skipped += 1;
            continue;
        }

        let color_space = image_color_space(stream)
            .expect("color space was validated")
            .to_vec();
        stream.dict.set("Filter", "DCTDecode");
        stream.dict.remove(b"DecodeParms");
        stream.dict.set("ColorSpace", Object::Name(color_space));
        stream.dict.set("BitsPerComponent", 8);
        stream.set_content(candidate);
        stats.recompressed += 1;
    }

    stats
}

fn mask_object_ids(document: &Document) -> HashSet<ObjectId> {
    let mut masks = HashSet::new();
    for object in document.objects.values() {
        let Ok(stream) = object.as_stream() else {
            continue;
        };
        if !is_image_stream(object) {
            continue;
        }
        for key in [b"SMask".as_slice(), b"Mask"] {
            if let Ok(target) = stream.dict.get(key).and_then(Object::as_reference) {
                masks.insert(target);
            }
        }
    }
    masks
}

fn encode_smaller_jpeg(stream: &Stream, quality: u8) -> Option<Vec<u8>> {
    if stream
        .dict
        .get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    if stream.dict.get(b"Mask").is_ok() || stream.dict.get(b"Decode").is_ok() {
        return None;
    }

    let width = stream.dict.get(b"Width").ok()?.as_i64().ok()?;
    let height = stream.dict.get(b"Height").ok()?.as_i64().ok()?;
    if width <= 0 || height <= 0 || width > u32::MAX as i64 || height > u32::MAX as i64 {
        return None;
    }
    let width = width as u32;
    let height = height as u32;
    let bits = stream.dict.get(b"BitsPerComponent").ok()?.as_i64().ok()?;
    if bits != 8 {
        return None;
    }

    let color_space = image_color_space(stream)?;
    let components = if color_space == b"DeviceRGB" { 3 } else { 1 };
    let expected = (width as u64)
        .checked_mul(height as u64)?
        .checked_mul(components)?;
    if expected > MAX_DECODED_IMAGE_BYTES as u64 {
        return None;
    }

    let filters = stream
        .filters()
        .ok()?
        .iter()
        .map(|filter| filter.to_vec())
        .collect::<Vec<_>>();
    if filters.len() != 1 {
        return None;
    }

    let mut encoded = Vec::new();
    match filters[0].as_slice() {
        b"DCTDecode" => {
            let mut limits = Limits::default();
            limits.max_image_width = Some(width);
            limits.max_image_height = Some(height);
            limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES as u64);

            let mut reader =
                ImageReader::with_format(Cursor::new(&stream.content), ImageFormat::Jpeg);
            reader.limits(limits);
            let decoded = reader.decode().ok()?;
            if decoded.width() != width || decoded.height() != height {
                return None;
            }
            encode_jpeg(&mut encoded, decoded, color_space, quality)?;
        }
        b"FlateDecode" => {
            let decoded = stream
                .decompressed_content_with_limit(MAX_DECODED_IMAGE_BYTES)
                .ok()?;
            if decoded.len() as u64 != expected {
                return None;
            }
            if color_space == b"DeviceRGB" {
                let image = RgbImage::from_raw(width, height, decoded)?;
                encode_jpeg(
                    &mut encoded,
                    DynamicImage::ImageRgb8(image),
                    color_space,
                    quality,
                )?;
            } else {
                let image = GrayImage::from_raw(width, height, decoded)?;
                encode_jpeg(
                    &mut encoded,
                    DynamicImage::ImageLuma8(image),
                    color_space,
                    quality,
                )?;
            }
        }
        _ => return None,
    }

    Some(encoded)
}

fn encode_jpeg(
    output: &mut Vec<u8>,
    image: DynamicImage,
    color_space: &[u8],
    quality: u8,
) -> Option<()> {
    let mut encoder = JpegEncoder::new_with_quality(output, quality);
    if color_space == b"DeviceRGB" {
        let image = image.to_rgb8();
        encoder
            .encode(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgb8,
            )
            .ok()?;
    } else {
        let image = image.to_luma8();
        encoder
            .encode(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::L8,
            )
            .ok()?;
    }
    Some(())
}

fn is_image_stream(object: &Object) -> bool {
    object
        .as_stream()
        .ok()
        .and_then(|stream| stream.dict.get(b"Subtype").and_then(Object::as_name).ok())
        .is_some_and(|name| name == b"Image")
}

fn image_color_space(stream: &Stream) -> Option<&[u8]> {
    let name = stream.dict.get(b"ColorSpace").ok()?.as_name().ok()?;
    if name == b"DeviceRGB" || name == b"DeviceGray" {
        Some(name)
    } else {
        None
    }
}

fn save_modern(document: &mut Document, input: &Path) -> Result<Vec<u8>, CompressError> {
    let mut bytes = Vec::new();
    document
        .save_with_options(
            &mut bytes,
            SaveOptions::builder()
                .use_object_streams(true)
                .use_xref_streams(true)
                .compression_level(9)
                .build(),
        )
        .map_err(|error| CompressError::InvalidPdf {
            path: input.to_path_buf(),
            reason: error.to_string(),
        })?;
    Ok(bytes)
}

fn write_safely(
    target: &Path,
    bytes: &[u8],
    allow_overwrite: bool,
    expected_pages: usize,
) -> Result<(), CompressError> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temp = NamedTempFile::new_in(parent).map_err(|source| CompressError::OutputWrite {
        path: target.to_path_buf(),
        source,
    })?;

    temp.write_all(bytes)
        .map_err(|source| CompressError::OutputWrite {
            path: target.to_path_buf(),
            source,
        })?;
    temp.as_file()
        .sync_all()
        .map_err(|source| CompressError::OutputWrite {
            path: target.to_path_buf(),
            source,
        })?;

    validate_written_pdf(temp.path(), expected_pages, target)?;

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
                Err(CompressError::OutputExists {
                    path: target.to_path_buf(),
                })
            } else {
                Err(CompressError::OutputWrite {
                    path: target.to_path_buf(),
                    source: error.error,
                })
            }
        }
    }
}

fn validate_written_pdf(
    temp: &Path,
    expected_pages: usize,
    target: &Path,
) -> Result<(), CompressError> {
    let document = Document::load(temp).map_err(|error| CompressError::OutputValidation {
        path: target.to_path_buf(),
        reason: error.to_string(),
    })?;
    if document.is_encrypted() {
        return Err(CompressError::OutputValidation {
            path: target.to_path_buf(),
            reason: "unexpected encryption".to_string(),
        });
    }
    let pages = document.get_pages().len();
    if pages != expected_pages {
        return Err(CompressError::OutputValidation {
            path: target.to_path_buf(),
            reason: format!("expected {expected_pages} pages, found {pages}"),
        });
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
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, Rgb, RgbImage};
    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::{EncryptionState, EncryptionVersion, Permissions, Stream, dictionary};
    use pdf_writer::types::{ActionType, AnnotationType};
    use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn flate(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
    }

    fn make_rgb(width: u32, height: u32, seed: u32) -> RgbImage {
        let mut image = RgbImage::new(width, height);
        let mut noise = 0x9e37_79b9u32;
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let grain = ((noise & 0xff) as i32 - 128) / 20;
            let fx = x as f32 / width as f32;
            let fy = y as f32 / height as f32;
            let wave = ((fx * 17.0 + seed as f32).sin() * (fy * 13.0).cos() * 42.0) as i32;
            *pixel = Rgb([
                ((90.0 + 120.0 * fx + 30.0 * fy) as i32 + wave + grain).clamp(0, 255) as u8,
                ((60.0 + 80.0 * (1.0 - fx) + 90.0 * fy) as i32 - wave + grain).clamp(0, 255) as u8,
                ((110.0 + 70.0 * fx * fy) as i32 + wave / 2 + grain).clamp(0, 255) as u8,
            ]);
        }
        image
    }

    fn encode_jpeg(image: &RgbImage, quality: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    fn alpha_mask(width: u32, height: u32) -> Vec<u8> {
        let mut data = vec![0u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                let fx = x as f32 / width as f32 * 2.0 - 1.0;
                let fy = y as f32 / height as f32 * 2.0 - 1.0;
                let radius = (fx * fx + fy * fy).sqrt();
                data[(y * width + x) as usize] = ((1.0 - radius) * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
        data
    }

    fn write_text_pdf(path: &Path, page_count: usize) {
        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let font = Ref::new(3);
        let page_refs: Vec<Ref> = (0..page_count)
            .map(|index| Ref::new((4 + index * 2) as i32))
            .collect();
        let content_refs: Vec<Ref> = (0..page_count)
            .map(|index| Ref::new((5 + index * 2) as i32))
            .collect();
        let annotation = Ref::new((4 + page_count * 2) as i32);
        let info = Ref::new((5 + page_count * 2) as i32);

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree)
            .kids(page_refs.iter().copied())
            .count(page_count as i32);

        for (index, page_ref) in page_refs.iter().enumerate() {
            let mut page = pdf.page(*page_ref);
            page.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
            page.parent(tree);
            page.contents(content_refs[index]);
            {
                let mut resources = page.resources();
                resources.fonts().pair(Name(b"F1"), font);
            }
            if index == 0 && page_count > 1 {
                page.annotations([annotation]);
            }
            if index == 1 {
                page.rotate(90);
            }
        }

        pdf.type1_font(font).base_font(Name(b"Helvetica"));
        pdf.document_info(info)
            .title(TextStr("Compression fixture"))
            .author(TextStr("Quicklet tests"))
            .producer(TextStr("Quicklet compression tests"));

        for content_ref in &content_refs {
            let mut content = Content::new();
            content.begin_text();
            content.set_font(Name(b"F1"), 10.0);
            content.next_line(44.0, 748.0);
            for index in 0..48 {
                content.show(Str(format!(
                    "Compression line {index:03}: the quick brown fox jumps over the lazy dog."
                )
                .as_bytes()));
                content.next_line(0.0, -14.0);
            }
            content.end_text();
            for index in 0..8 {
                content.set_stroke_rgb(0.2, 0.35, 0.65);
                content.rect(44.0 + index as f32 * 8.0, 70.0, 120.0, 20.0);
                content.stroke();
            }
            pdf.stream(*content_ref, &content.finish());
        }

        if page_count > 1 {
            let mut link = pdf.annotation(annotation);
            link.subtype(AnnotationType::Link);
            link.rect(Rect::new(44.0, 700.0, 220.0, 720.0));
            link.action()
                .action_type(ActionType::GoTo)
                .destination()
                .page(page_refs[1])
                .fit();
            link.finish();
        }

        fs::write(path, pdf.finish()).unwrap();
    }

    fn write_image_pdf(path: &Path, jpeg_quality: u8, with_smask: bool) {
        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let font = Ref::new(3);
        let page1 = Ref::new(4);
        let content1 = Ref::new(5);
        let jpeg_image = Ref::new(6);
        let page2 = Ref::new(7);
        let content2 = Ref::new(8);
        let flate_image = Ref::new(9);
        let smask = Ref::new(10);
        let annotation = Ref::new(11);
        let info = Ref::new(12);

        let image = make_rgb(640, 420, 3);
        let jpeg = encode_jpeg(&image, jpeg_quality);

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree).kids([page1, page2]).count(2);

        {
            let mut page = pdf.page(page1);
            page.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
            page.parent(tree);
            page.contents(content1);
            page.rotate(0);
            {
                let mut resources = page.resources();
                resources.fonts().pair(Name(b"F1"), font);
                resources.x_objects().pair(Name(b"Im1"), jpeg_image);
            }
            page.annotations([annotation]);
        }
        {
            let mut page = pdf.page(page2);
            page.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
            page.parent(tree);
            page.contents(content2);
            page.rotate(90);
            let mut resources = page.resources();
            resources.fonts().pair(Name(b"F1"), font);
            resources.x_objects().pair(Name(b"Im2"), flate_image);
        }

        pdf.type1_font(font).base_font(Name(b"Helvetica"));
        pdf.document_info(info)
            .title(TextStr("Image compression fixture"))
            .producer(TextStr("Quicklet compression tests"));

        let mut writer = pdf.image_xobject(jpeg_image, &jpeg);
        writer.filter(Filter::DctDecode);
        writer.width(image.width() as i32);
        writer.height(image.height() as i32);
        writer.color_space().device_rgb();
        writer.bits_per_component(8);
        writer.finish();

        let mask = alpha_mask(image.width(), image.height());
        let mask_data = flate(&mask);
        let flate_data = flate(image.as_raw());
        let mut writer = pdf.image_xobject(flate_image, &flate_data);
        writer.filter(Filter::FlateDecode);
        writer.width(image.width() as i32);
        writer.height(image.height() as i32);
        writer.color_space().device_rgb();
        writer.bits_per_component(8);
        if with_smask {
            writer.s_mask(smask);
        }
        writer.finish();

        if with_smask {
            let mut writer = pdf.image_xobject(smask, &mask_data);
            writer.filter(Filter::FlateDecode);
            writer.width(image.width() as i32);
            writer.height(image.height() as i32);
            writer.color_space().device_gray();
            writer.bits_per_component(8);
            writer.finish();
        }

        for (content_ref, image_name, label) in [
            (content1, Name(b"Im1"), "JPEG page"),
            (content2, Name(b"Im2"), "Flate page"),
        ] {
            let mut content = Content::new();
            content.begin_text();
            content.set_font(Name(b"F1"), 16.0);
            content.next_line(44.0, 740.0);
            content.show(Str(label.as_bytes()));
            content.end_text();
            content.save_state();
            content.transform([480.0, 0.0, 0.0, 315.0, 60.0, 380.0]);
            content.x_object(image_name);
            content.restore_state();
            pdf.stream(content_ref, &content.finish());
        }

        let mut link = pdf.annotation(annotation);
        link.subtype(AnnotationType::Link);
        link.rect(Rect::new(44.0, 700.0, 220.0, 720.0));
        link.action()
            .action_type(ActionType::GoTo)
            .destination()
            .page(page2)
            .fit();
        link.finish();

        fs::write(path, pdf.finish()).unwrap();
    }

    fn write_jpeg_pair_pdf(path: &Path, quality: u8) {
        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let font = Ref::new(3);
        let page1 = Ref::new(4);
        let content1 = Ref::new(5);
        let image1 = Ref::new(6);
        let page2 = Ref::new(7);
        let content2 = Ref::new(8);
        let image2 = Ref::new(9);
        let info = Ref::new(10);

        let first = make_rgb(640, 420, 5);
        let second = make_rgb(640, 420, 9);
        let first_data = encode_jpeg(&first, quality);
        let second_data = encode_jpeg(&second, quality);

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree).kids([page1, page2]).count(2);
        for (page_ref, content_ref, image_ref, image) in [
            (page1, content1, image1, &first),
            (page2, content2, image2, &second),
        ] {
            let mut page = pdf.page(page_ref);
            page.media_box(Rect::new(0.0, 0.0, 612.0, 792.0));
            page.parent(tree);
            page.contents(content_ref);
            let mut resources = page.resources();
            resources.fonts().pair(Name(b"F1"), font);
            resources.x_objects().pair(Name(b"Im1"), image_ref);
            let _ = image;
        }

        pdf.type1_font(font).base_font(Name(b"Helvetica"));
        pdf.document_info(info)
            .title(TextStr("JPEG pair"))
            .producer(TextStr("Quicklet compression tests"));

        for (image_ref, data, image) in [
            (image1, &first_data, &first),
            (image2, &second_data, &second),
        ] {
            let mut writer = pdf.image_xobject(image_ref, data);
            writer.filter(Filter::DctDecode);
            writer.width(image.width() as i32);
            writer.height(image.height() as i32);
            writer.color_space().device_rgb();
            writer.bits_per_component(8);
            writer.finish();
        }

        for (content_ref, name) in [(content1, Name(b"Im1")), (content2, Name(b"Im1"))] {
            let mut content = Content::new();
            content.save_state();
            content.transform([480.0, 0.0, 0.0, 315.0, 60.0, 380.0]);
            content.x_object(name);
            content.restore_state();
            pdf.stream(content_ref, &content.finish());
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
        let page_numbers: BTreeMap<ObjectId, u32> = document
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

    fn image_ids(document: &Document) -> Vec<ObjectId> {
        document
            .objects
            .iter()
            .filter_map(|(id, object)| is_image_stream(object).then_some(*id))
            .collect()
    }

    fn image_count(document: &Document) -> usize {
        image_ids(document).len()
    }

    fn smask_ids(document: &Document) -> Vec<ObjectId> {
        document
            .objects
            .values()
            .filter_map(|object| {
                object
                    .as_stream()
                    .ok()
                    .and_then(|stream| stream.dict.get(b"SMask").ok())
                    .and_then(|value| value.as_reference().ok())
            })
            .collect()
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

    fn write_structural_pdf(path: &Path) {
        write_text_pdf(path, 3);
        let mut document = Document::load(path).unwrap();
        let page_id = page_ids(&document)[0];
        let catalog_id = document
            .trailer
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();

        let outlines_id = document.new_object_id();
        let outline_item = document.add_object(dictionary! {
            "Title" => Object::string_literal("Chapter One"),
            "Parent" => outlines_id,
            "Dest" => vec![Object::Reference(page_id), Object::Name(b"Fit".to_vec())],
        });
        document.objects.insert(
            outlines_id,
            Object::Dictionary(dictionary! {
                "Type" => "Outlines",
                "First" => outline_item,
                "Last" => outline_item,
                "Count" => 1,
            }),
        );

        let text_field = document.add_object(dictionary! {
            "FT" => "Tx",
            "T" => Object::string_literal("Name"),
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => vec![44.into(), 500.into(), 244.into(), 520.into()],
            "P" => page_id,
            "F" => 4,
            "V" => Object::string_literal("Test value"),
        });
        let acro_form = document.add_object(dictionary! {
            "Fields" => vec![Object::Reference(text_field)],
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
            b"<x:xmpmeta>Quicklet compression metadata</x:xmpmeta>".to_vec(),
        ));
        let icc_profile = document.add_object(Stream::new(
            dictionary! { "N" => 3 },
            b"fake ICC profile bytes".to_vec(),
        ));
        let icc_image = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 2,
                "Height" => 2,
                "BitsPerComponent" => 8,
                "ColorSpace" => vec![
                    Object::Name(b"ICCBased".to_vec()),
                    Object::Reference(icc_profile),
                ],
                "Filter" => "FlateDecode",
            },
            flate(&[255u8; 2 * 2 * 3]),
        ));

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

        {
            let page = document
                .get_object_mut(page_id)
                .unwrap()
                .as_dict_mut()
                .unwrap();
            let resources = page.get_mut(b"Resources").unwrap().as_dict_mut().unwrap();
            resources.set("XObject", dictionary! { "ImICC" => icc_image });
            let annots = page.get_mut(b"Annots").unwrap().as_array_mut().unwrap();
            annots.push(Object::Reference(text_field));
        }

        document.save(path).unwrap();
    }

    fn add_signature(path: &Path) {
        let mut document = Document::load(path).unwrap();
        let page_id = page_ids(&document)[0];
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
            "Rect" => vec![44.into(), 530.into(), 244.into(), 550.into()],
            "P" => page_id,
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
        let file_id = Object::string_literal(b"quicklet-compress-test");
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

    fn source_bytes(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()
    }

    #[test]
    fn light_compresses_text_and_preserves_pages_text_links_and_info() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("text.pdf");
        let output = dir.path().join("text-light.pdf");
        write_text_pdf(&input, 5);
        let original = source_bytes(&input);

        let outcome = compress_pdf(&input, &output, CompressionPreset::Light).unwrap();
        assert_eq!(outcome.status, CompressionStatus::Reduced);
        assert!(outcome.output_bytes < outcome.original_bytes);
        assert!(outcome.reduction_percent > 50.0);
        assert_eq!(outcome.images_recompressed, 0);

        let document = Document::load(&output).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 5);
        assert!(page_text(&document, ids[0]).contains("Compression line 000"));
        assert_eq!(media_box(&document, ids[1]), (612.0, 792.0));
        assert_eq!(rotation(&document, ids[1]), 90);
        assert_eq!(link_targets(&document, ids[0]), vec![2]);
        assert_eq!(
            info_value(&document, b"Title").as_deref(),
            Some(b"Compression fixture".as_slice())
        );
        assert_eq!(font_count(&document), 1);
        assert_eq!(source_bytes(&input), original);
    }

    #[test]
    fn balanced_and_strong_reduce_images_and_preserve_masks() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("images.pdf");
        let balanced = dir.path().join("balanced.pdf");
        let strong = dir.path().join("strong.pdf");
        write_image_pdf(&input, 95, true);
        let original_document = Document::load(&input).unwrap();
        let original_masks = smask_ids(&original_document);
        let original_mask_bytes = original_masks
            .iter()
            .map(|id| {
                original_document
                    .get_object(*id)
                    .unwrap()
                    .as_stream()
                    .unwrap()
                    .content
                    .clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(original_masks.len(), 1);

        let balanced_outcome =
            compress_pdf(&input, &balanced, CompressionPreset::Balanced).unwrap();
        let strong_outcome = compress_pdf(&input, &strong, CompressionPreset::Strong).unwrap();

        assert_eq!(balanced_outcome.status, CompressionStatus::Reduced);
        assert_eq!(strong_outcome.status, CompressionStatus::Reduced);
        assert!(strong_outcome.output_bytes < balanced_outcome.output_bytes);
        assert!(balanced_outcome.images_recompressed >= 2);
        assert!(balanced_outcome.images_skipped >= 1);
        assert!(strong_outcome.images_recompressed >= 2);

        let original = Document::load(&input).unwrap();
        for output in [&balanced, &strong] {
            let document = Document::load(output).unwrap();
            let ids = page_ids(&document);
            assert_eq!(ids.len(), 2);
            assert_eq!(media_box(&document, ids[1]), (612.0, 792.0));
            assert_eq!(rotation(&document, ids[1]), 90);
            assert!(page_text(&document, ids[0]).contains("JPEG page"));
            assert_eq!(image_count(&document), image_count(&original));
            assert_eq!(smask_ids(&document), original_masks);

            for (index, mask_id) in original_masks.iter().enumerate() {
                let mask = document.get_object(*mask_id).unwrap().as_stream().unwrap();
                assert_eq!(mask.content, original_mask_bytes[index]);
                assert_eq!(
                    mask.dict.get(b"Filter").unwrap().as_name().unwrap(),
                    b"FlateDecode"
                );
            }

            let jpeg_streams = image_ids(&document)
                .iter()
                .filter_map(|id| document.get_object(*id).unwrap().as_stream().ok())
                .filter(|stream| {
                    stream
                        .dict
                        .get(b"Filter")
                        .and_then(Object::as_name)
                        .is_ok_and(|name| name == b"DCTDecode")
                })
                .count();
            assert!(jpeg_streams >= 2);
        }
    }

    #[test]
    fn all_presets_produce_valid_image_and_mixed_outputs() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("mixed.pdf");
        write_image_pdf(&input, 95, true);

        for (index, preset) in [
            CompressionPreset::Light,
            CompressionPreset::Balanced,
            CompressionPreset::Strong,
        ]
        .into_iter()
        .enumerate()
        {
            let output = dir.path().join(format!("mixed-{index}.pdf"));
            let outcome = compress_pdf(&input, &output, preset).unwrap();
            assert!(outcome.output_bytes > 0);
            let document = Document::load(&output).unwrap();
            assert_eq!(page_ids(&document).len(), 2);
            assert_eq!(
                image_count(&document),
                image_count(&Document::load(&input).unwrap())
            );
            if preset == CompressionPreset::Light {
                assert_eq!(outcome.images_recompressed, 0);
            } else {
                assert!(outcome.images_recompressed >= 1);
            }
        }
    }

    #[test]
    fn threshold_skips_already_compressed_images_and_retains_original_bytes() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let light_input = dir.path().join("light-input.pdf");
        let balanced = dir.path().join("balanced.pdf");
        let strong = dir.path().join("strong.pdf");
        write_jpeg_pair_pdf(&source, 50);

        compress_pdf(&source, &light_input, CompressionPreset::Light).unwrap();
        let original = source_bytes(&light_input);

        for (preset, output) in [
            (CompressionPreset::Balanced, &balanced),
            (CompressionPreset::Strong, &strong),
        ] {
            let outcome = compress_pdf(&light_input, output, preset).unwrap();
            assert_eq!(outcome.status, CompressionStatus::NoReduction);
            assert_eq!(outcome.saved_bytes, 0);
            assert_eq!(outcome.reduction_percent, 0.0);
            assert_eq!(outcome.images_recompressed, 0);
            assert_eq!(source_bytes(output), original);
            let document = Document::load(output).unwrap();
            assert_eq!(page_ids(&document).len(), 2);
        }
    }

    #[test]
    fn text_only_balanced_and_strong_match_light() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("text.pdf");
        let light = dir.path().join("light.pdf");
        let balanced = dir.path().join("balanced.pdf");
        let strong = dir.path().join("strong.pdf");
        write_text_pdf(&input, 3);

        compress_pdf(&input, &light, CompressionPreset::Light).unwrap();
        compress_pdf(&input, &balanced, CompressionPreset::Balanced).unwrap();
        compress_pdf(&input, &strong, CompressionPreset::Strong).unwrap();

        assert_eq!(source_bytes(&light), source_bytes(&balanced));
        assert_eq!(source_bytes(&light), source_bytes(&strong));
    }

    #[test]
    fn rejects_unsupported_and_oversized_images_without_failing_light() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("unsupported.pdf");
        let output = dir.path().join("out.pdf");
        write_image_pdf(&input, 95, false);

        let mut document = Document::load(&input).unwrap();
        let jpx = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 10,
                "Height" => 10,
                "BitsPerComponent" => 8,
                "ColorSpace" => "DeviceRGB",
                "Filter" => "JPXDecode",
            },
            b"not a jpeg2000 stream".to_vec(),
        ));
        let huge = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 100_000,
                "Height" => 100_000,
                "BitsPerComponent" => 8,
                "ColorSpace" => "DeviceRGB",
                "Filter" => "FlateDecode",
            },
            flate(b"tiny"),
        ));
        assert!(jpx.0 > 0 && huge.0 > 0);
        document.save(&input).unwrap();

        let outcome = compress_pdf(&input, &output, CompressionPreset::Balanced).unwrap();
        assert_eq!(outcome.status, CompressionStatus::Reduced);
        assert!(outcome.images_skipped >= 2);
        let output_document = Document::load(&output).unwrap();
        assert_eq!(page_ids(&output_document).len(), 2);
    }

    #[test]
    fn skips_font_program_streams() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("font.pdf");
        let output = dir.path().join("out.pdf");
        write_text_pdf(&input, 1);

        let mut document = Document::load(&input).unwrap();
        let font_stream = document.add_object(Stream::new(Dictionary::new(), b"font!".to_vec()));
        let _descriptor = document.add_object(dictionary! {
            "Type" => "FontDescriptor",
            "FontName" => "QuickletTest",
            "FontFile2" => font_stream,
        });
        document.save(&input).unwrap();
        assert!(font_stream.0 > 0);

        compress_pdf(&input, &output, CompressionPreset::Light).unwrap();
        let output_document = Document::load(&output).unwrap();
        let stream = output_document
            .get_object(font_stream)
            .unwrap()
            .as_stream()
            .unwrap();
        assert!(stream.dict.get(b"Filter").is_err());
        assert_eq!(stream.content, b"font!");
    }

    #[test]
    fn preserves_bookmarks_forms_tagged_structure_metadata_and_icc() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("structure.pdf");
        let output = dir.path().join("structure-out.pdf");
        write_structural_pdf(&input);

        compress_pdf(&input, &output, CompressionPreset::Balanced).unwrap();
        let original = Document::load(&input).unwrap();
        let document = Document::load(&output).unwrap();
        let ids = page_ids(&document);
        assert_eq!(ids.len(), 3);
        assert_eq!(page_ids(&original).len(), 3);
        assert_eq!(annotation_count(&document), annotation_count(&original));
        assert!(page_text(&document, ids[0]).contains("Compression line"));
        assert_eq!(link_targets(&document, ids[0]), vec![2]);
        assert_eq!(
            info_value(&document, b"Author").as_deref(),
            Some(b"Quicklet tests".as_slice())
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
        let item_id = outlines.get(b"First").unwrap().as_reference().unwrap();
        let item = document.get_object(item_id).unwrap().as_dict().unwrap();
        assert_eq!(
            item.get(b"Title").unwrap().as_str().unwrap(),
            b"Chapter One"
        );

        let metadata_id = catalog.get(b"Metadata").unwrap().as_reference().unwrap();
        let metadata = document
            .get_object(metadata_id)
            .unwrap()
            .as_stream()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&metadata.content).contains("Quicklet compression metadata")
        );

        let acro_id = catalog.get(b"AcroForm").unwrap().as_reference().unwrap();
        let acro = document.get_object(acro_id).unwrap().as_dict().unwrap();
        assert!(acro.get(b"Fields").is_ok());
        assert_eq!(
            document
                .catalog()
                .unwrap()
                .get(b"PageLabels")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"Nums")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let icc_images = image_ids(&document);
        assert!(!icc_images.is_empty());
        let icc_image = icc_images
            .iter()
            .find_map(|id| {
                let stream = document.get_object(*id).unwrap().as_stream().ok()?;
                let color_space = stream.dict.get(b"ColorSpace").ok()?.as_array().ok()?;
                (color_space.first()?.as_name().ok()? == b"ICCBased").then_some(stream)
            })
            .expect("ICCBased image should remain");
        assert_eq!(
            icc_image.dict.get(b"Filter").unwrap().as_name().unwrap(),
            b"FlateDecode"
        );
    }

    #[test]
    fn detects_digital_signatures() {
        let dir = tempdir().unwrap();
        let plain = dir.path().join("plain.pdf");
        let signed = dir.path().join("signed.pdf");
        write_text_pdf(&plain, 1);
        fs::copy(&plain, &signed).unwrap();
        add_signature(&signed);

        assert!(!inspect_pdf(&plain).unwrap().has_signature);
        assert!(inspect_pdf(&signed).unwrap().has_signature);
    }

    #[test]
    fn rejects_encrypted_malformed_and_invalid_references() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.pdf");
        let output = dir.path().join("out.pdf");
        write_text_pdf(&source, 1);

        let encrypted = dir.path().join("encrypted.pdf");
        write_encrypted_pdf(&source, &encrypted);
        assert!(matches!(
            inspect_pdf(&encrypted),
            Err(CompressError::EncryptedPdf { .. })
        ));

        let truncated = dir.path().join("truncated.pdf");
        let bytes = source_bytes(&source);
        fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            inspect_pdf(&truncated),
            Err(CompressError::InvalidPdf { .. })
        ));

        let broken = dir.path().join("broken.pdf");
        let mut document = Document::load(&source).unwrap();
        let page_id = page_ids(&document)[0];
        page_dictionary(&document, page_id); // ensure page exists
        document
            .get_object_mut(page_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Contents", Object::Reference((9999, 0)));
        document.save(&broken).unwrap();
        assert!(matches!(
            inspect_pdf(&broken),
            Err(CompressError::InvalidReference { .. })
        ));
        assert!(!output.exists());
    }

    #[test]
    fn rejects_same_path_and_collisions_and_preserves_existing_files() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        let output = dir.path().join("output.pdf");
        write_text_pdf(&input, 1);
        let original = source_bytes(&input);

        assert!(matches!(
            compress_pdf(&input, &input, CompressionPreset::Light),
            Err(CompressError::SameAsInput { .. })
        ));
        assert_eq!(source_bytes(&input), original);

        fs::write(&output, b"keep me").unwrap();
        assert!(matches!(
            compress_pdf(&input, &output, CompressionPreset::Light),
            Err(CompressError::OutputExists { .. })
        ));
        assert_eq!(source_bytes(&output), b"keep me");
        assert_eq!(source_bytes(&input), original);

        compress_pdf_with_overwrite(&input, &output, CompressionPreset::Light).unwrap();
        assert_ne!(source_bytes(&output), b"keep me");
        assert_eq!(source_bytes(&input), original);
        assert_eq!(page_ids(&Document::load(&output).unwrap()).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_hard_link_destination() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        let linked = dir.path().join("linked.pdf");
        write_text_pdf(&input, 1);
        fs::hard_link(&input, &linked).unwrap();

        assert!(matches!(
            compress_pdf(&input, &linked, CompressionPreset::Light),
            Err(CompressError::SameAsInput { .. })
        ));
    }

    #[test]
    fn validates_temp_output_and_cleans_up_on_invalid_pdf() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("invalid.pdf");

        let error = write_safely(&target, b"not a pdf", false, 1).unwrap_err();
        assert!(matches!(error, CompressError::OutputValidation { .. }));
        assert!(!target.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn enforces_input_page_and_image_limits() {
        let dir = tempdir().unwrap();
        let sparse = dir.path().join("sparse.pdf");
        let file = fs::File::create(&sparse).unwrap();
        file.set_len(MAX_COMPRESS_INPUT_BYTES + 1).unwrap();
        assert!(matches!(
            inspect_pdf(&sparse),
            Err(CompressError::InputTooLarge { .. })
        ));

        let many_pages = dir.path().join("many-pages.pdf");
        let mut document = Document::with_version("1.5");
        let tree = document.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..=MAX_COMPRESS_PAGES {
            let page = document.new_object_id();
            document.objects.insert(
                page,
                Object::Dictionary(dictionary! {
                    "Type" => "Page",
                    "Parent" => tree,
                    "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
                }),
            );
            kids.push(Object::Reference(page));
        }
        document.objects.insert(
            tree,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => (MAX_COMPRESS_PAGES + 1) as i64,
            }),
        );
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => tree,
        });
        document.trailer.set("Root", catalog);
        document.save(&many_pages).unwrap();
        assert!(matches!(
            inspect_pdf(&many_pages),
            Err(CompressError::TooManyPages { .. })
        ));

        let many_images = dir.path().join("many-images.pdf");
        let mut document = Document::with_version("1.5");
        let tree = document.new_object_id();
        let page = document.new_object_id();
        document.objects.insert(
            page,
            Object::Dictionary(dictionary! {
                "Type" => "Page",
                "Parent" => tree,
                "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            }),
        );
        document.objects.insert(
            tree,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page)],
                "Count" => 1,
            }),
        );
        for _ in 0..=MAX_COMPRESS_IMAGE_OBJECTS {
            document.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => 1,
                    "Height" => 1,
                    "BitsPerComponent" => 8,
                    "ColorSpace" => "DeviceRGB",
                    "Filter" => "FlateDecode",
                },
                flate(b"x"),
            ));
        }
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => tree,
        });
        document.trailer.set("Root", catalog);
        document.save(&many_images).unwrap();
        assert!(matches!(
            inspect_pdf(&many_images),
            Err(CompressError::TooManyImages { .. })
        ));
    }

    #[test]
    fn repeated_compression_is_safe() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.pdf");
        let first = dir.path().join("first.pdf");
        let second = dir.path().join("second.pdf");
        write_text_pdf(&input, 2);

        compress_pdf(&input, &first, CompressionPreset::Light).unwrap();
        let second_outcome = compress_pdf(&first, &second, CompressionPreset::Light).unwrap();
        assert_eq!(page_ids(&Document::load(&second).unwrap()).len(), 2);
        assert!(
            second_outcome.status == CompressionStatus::NoReduction
                || second_outcome.output_bytes <= second_outcome.original_bytes
        );
    }
}
