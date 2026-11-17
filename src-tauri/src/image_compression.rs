//! Lossless and quality-controlled compression for one PNG, JPEG or WebP.
//!
//! Compression never changes the format, dimensions or alpha channel. ICC and
//! EXIF metadata are preserved; XMP, IPTC, PNG text, gamma/chromaticity and
//! other ancillary metadata are not preserved. EXIF orientation is left in the
//! metadata and is never applied to the stored pixels.
//!
//! If the re-encoded candidate is not smaller than the source, the exact
//! original bytes are copied to the destination instead.

use std::fmt;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader};
use tempfile::NamedTempFile;

use crate::image_conversion::{self, DEFAULT_JPEG_QUALITY, ImageConversionError, ImageFormat};
use crate::progress::{self, Progress};

pub const DEFAULT_COMPRESSION_JPEG_QUALITY: u8 = DEFAULT_JPEG_QUALITY;
pub const MAX_PRESERVED_METADATA_BYTES: usize = 16 * 1024 * 1024;
const COPY_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompressionStatus {
    Reduced,
    NoReduction,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageMetadata {
    pub icc: Option<Vec<u8>>,
    pub exif: Option<Vec<u8>>,
    pub has_xmp: bool,
    pub has_iptc: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageMetadataSummary {
    pub has_icc: bool,
    pub has_exif: bool,
    pub has_xmp: bool,
    pub has_iptc: bool,
    pub preserved_bytes: usize,
}

impl ImageMetadata {
    fn summary(&self) -> ImageMetadataSummary {
        ImageMetadataSummary {
            has_icc: self.icc.is_some(),
            has_exif: self.exif.is_some(),
            has_xmp: self.has_xmp,
            has_iptc: self.has_iptc,
            preserved_bytes: self.icc.as_ref().map_or(0, Vec::len)
                + self.exif.as_ref().map_or(0, Vec::len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompressionSource {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub has_alpha: bool,
    pub orientation: u8,
    pub metadata: ImageMetadataSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressionOptions {
    pub jpeg_quality: u8,
}

impl CompressionOptions {
    pub fn new() -> Self {
        Self {
            jpeg_quality: DEFAULT_COMPRESSION_JPEG_QUALITY,
        }
    }
}

impl Default for CompressionOptions {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompressionOutcome {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub original_bytes: u64,
    pub output_bytes: u64,
    pub saved_bytes: u64,
    pub reduction_percent: f64,
    pub status: CompressionStatus,
    pub had_alpha: bool,
    pub metadata: ImageMetadataSummary,
}

#[derive(Debug)]
pub enum ImageCompressionError {
    Image(ImageConversionError),
    InputRead { path: PathBuf, source: io::Error },
    MetadataRead { path: PathBuf, reason: String },
    MetadataTooLarge { bytes: usize, max: usize },
    MetadataWrite { reason: String },
    InvalidQuality { quality: u8 },
    UnsupportedColor { format: ImageFormat },
    Encode { format: ImageFormat, reason: String },
    OutputExists { path: PathBuf },
    OutputValidation { path: PathBuf, reason: String },
    OutputWrite { path: PathBuf, source: io::Error },
    Cancelled,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl fmt::Display for ImageCompressionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image(error) => write!(formatter, "{error}"),
            Self::InputRead { path, .. } => {
                write!(formatter, "Could not read {}.", file_name(path))
            }
            Self::MetadataRead { path, .. } => {
                write!(
                    formatter,
                    "Could not read metadata from {}.",
                    file_name(path)
                )
            }
            Self::MetadataTooLarge { bytes, max } => write!(
                formatter,
                "ICC and EXIF metadata is too large to preserve ({:.1} MB; limit {:.1} MB).",
                *bytes as f64 / (1024.0 * 1024.0),
                *max as f64 / (1024.0 * 1024.0)
            ),
            Self::MetadataWrite { .. } => {
                write!(formatter, "Could not write image metadata.")
            }
            Self::InvalidQuality { quality } => write!(
                formatter,
                "JPEG quality must be between 1 and 100 (got {quality})."
            ),
            Self::UnsupportedColor { format } => write!(
                formatter,
                "{} output does not support this image's pixel format.",
                format.label()
            ),
            Self::Encode { format, .. } => {
                write!(formatter, "Could not encode the {} image.", format.label())
            }
            Self::OutputExists { path } => write!(
                formatter,
                "A file named \"{}\" already exists. Choose a different name or folder.",
                file_name(path)
            ),
            Self::OutputValidation { path, reason } => write!(
                formatter,
                "The compressed image could not be verified, so {} was not written: {reason}",
                file_name(path)
            ),
            Self::OutputWrite { path, .. } => {
                write!(formatter, "Could not write {}.", file_name(path))
            }
            Self::Cancelled => write!(formatter, "Compression cancelled."),
        }
    }
}

impl std::error::Error for ImageCompressionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Image(error) => Some(error),
            Self::InputRead { source, .. } | Self::OutputWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<ImageConversionError> for ImageCompressionError {
    fn from(error: ImageConversionError) -> Self {
        Self::Image(error)
    }
}

impl From<progress::Cancelled> for ImageCompressionError {
    fn from(_: progress::Cancelled) -> Self {
        Self::Cancelled
    }
}

impl progress::OperationError for ImageCompressionError {
    fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
            || matches!(self, Self::Image(ImageConversionError::Cancelled))
    }
}

/// Reads the header and metadata only; no pixels are decoded.
pub fn inspect_image(input: &Path) -> Result<CompressionSource, ImageCompressionError> {
    let bytes = image_conversion::read_input(input)?;
    let (mut decoder, source) = image_conversion::open_decoder(input, bytes)?;
    let metadata = capture_metadata(&mut decoder, input)?;

    Ok(CompressionSource {
        format: source.format,
        width: source.width,
        height: source.height,
        bytes: source.bytes,
        has_alpha: source.has_alpha,
        orientation: source.orientation,
        metadata: metadata.summary(),
    })
}

/// Compresses one image without overwriting an existing destination.
pub fn compress_image(
    input: &Path,
    output: &Path,
    options: CompressionOptions,
) -> Result<CompressionOutcome, ImageCompressionError> {
    compress_image_with_progress(input, output, options, false, &mut ())
}

/// Compresses one image and atomically replaces an existing destination.
///
/// Use only after explicit confirmation in a native save dialog.
pub fn compress_image_with_overwrite(
    input: &Path,
    output: &Path,
    options: CompressionOptions,
) -> Result<CompressionOutcome, ImageCompressionError> {
    compress_image_with_progress(input, output, options, true, &mut ())
}

pub fn compress_image_with_progress(
    input: &Path,
    output: &Path,
    options: CompressionOptions,
    allow_overwrite: bool,
    progress: &mut dyn Progress,
) -> Result<CompressionOutcome, ImageCompressionError> {
    progress.step("reading-image", 0, 0)?;
    let bytes = image_conversion::read_input(input)?;
    let (mut decoder, source) = image_conversion::open_decoder(input, bytes)?;

    if source.format == ImageFormat::Jpeg && !(1..=100).contains(&options.jpeg_quality) {
        return Err(ImageCompressionError::InvalidQuality {
            quality: options.jpeg_quality,
        });
    }

    progress.step("reading-metadata", 0, 0)?;
    let metadata = capture_metadata(&mut decoder, input)?;
    image_conversion::validate_output(input, output, allow_overwrite)?;

    progress.step("decoding", 0, 0)?;
    let image = DynamicImage::from_decoder(decoder).map_err(|error| {
        ImageCompressionError::Image(ImageConversionError::InvalidImage {
            path: input.to_path_buf(),
            reason: error.to_string(),
        })
    })?;
    let (width, height) = (image.width(), image.height());

    progress.step("encoding", 0, 0)?;
    let encoded = encode(&image, source.format, options, &metadata)?;
    drop(image);

    progress.step("validating-image", 0, 0)?;
    validate_candidate(
        output,
        &encoded,
        source.format,
        width,
        height,
        source.has_alpha,
        &metadata,
    )?;

    let original_size = source.bytes;
    let metadata_summary = metadata.summary();
    if encoded.len() as u64 >= original_size {
        drop(encoded);
        progress.step("saving-image", 0, 0)?;
        copy_safely(input, output, allow_overwrite, progress)?;
        return Ok(CompressionOutcome {
            format: source.format,
            width,
            height,
            original_bytes: original_size,
            output_bytes: original_size,
            saved_bytes: 0,
            reduction_percent: 0.0,
            status: CompressionStatus::NoReduction,
            had_alpha: source.has_alpha,
            metadata: metadata_summary,
        });
    }

    progress.step("saving-image", 0, 0)?;
    image_conversion::write_safely(output, &encoded, allow_overwrite)?;
    let output_size = encoded.len() as u64;
    let saved_bytes = original_size.saturating_sub(output_size);
    let reduction_percent = saved_bytes as f64 / original_size as f64 * 100.0;

    Ok(CompressionOutcome {
        format: source.format,
        width,
        height,
        original_bytes: original_size,
        output_bytes: output_size,
        saved_bytes,
        reduction_percent,
        status: CompressionStatus::Reduced,
        had_alpha: source.has_alpha,
        metadata: metadata_summary,
    })
}

fn capture_metadata<D: ImageDecoder>(
    decoder: &mut D,
    path: &Path,
) -> Result<ImageMetadata, ImageCompressionError> {
    let icc = decoder
        .icc_profile()
        .map_err(|error| ImageCompressionError::MetadataRead {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let exif = decoder
        .exif_metadata()
        .map_err(|error| ImageCompressionError::MetadataRead {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;

    let preserved_bytes = icc.as_ref().map_or(0, Vec::len) + exif.as_ref().map_or(0, Vec::len);
    if preserved_bytes > MAX_PRESERVED_METADATA_BYTES {
        return Err(ImageCompressionError::MetadataTooLarge {
            bytes: preserved_bytes,
            max: MAX_PRESERVED_METADATA_BYTES,
        });
    }

    let has_xmp = decoder.xmp_metadata().ok().flatten().is_some();
    let has_iptc = decoder.iptc_metadata().ok().flatten().is_some();

    Ok(ImageMetadata {
        icc,
        exif,
        has_xmp,
        has_iptc,
    })
}

fn apply_metadata<E: ImageEncoder>(
    encoder: &mut E,
    metadata: &ImageMetadata,
) -> Result<(), ImageCompressionError> {
    if let Some(icc) = &metadata.icc {
        encoder.set_icc_profile(icc.clone()).map_err(|error| {
            ImageCompressionError::MetadataWrite {
                reason: error.to_string(),
            }
        })?;
    }
    if let Some(exif) = &metadata.exif {
        encoder.set_exif_metadata(exif.clone()).map_err(|error| {
            ImageCompressionError::MetadataWrite {
                reason: error.to_string(),
            }
        })?;
    }
    Ok(())
}

fn encode(
    image: &DynamicImage,
    format: ImageFormat,
    options: CompressionOptions,
    metadata: &ImageMetadata,
) -> Result<Vec<u8>, ImageCompressionError> {
    let (width, height) = (image.width(), image.height());
    let mut encoded = Vec::new();
    let (data, color) = (image.as_bytes(), image.color().into());
    match format {
        ImageFormat::Png => {
            let mut encoder = PngEncoder::new_with_quality(
                &mut encoded,
                CompressionType::Level(9),
                FilterType::Adaptive,
            );
            apply_metadata(&mut encoder, metadata)?;
            encoder
                .write_image(data, width, height, color)
                .map_err(|error| ImageCompressionError::Encode {
                    format,
                    reason: error.to_string(),
                })?;
        }
        ImageFormat::Jpeg => {
            if !matches!(color, ExtendedColorType::L8 | ExtendedColorType::Rgb8) {
                return Err(ImageCompressionError::UnsupportedColor { format });
            }
            let mut encoder = JpegEncoder::new_with_quality(&mut encoded, options.jpeg_quality);
            apply_metadata(&mut encoder, metadata)?;
            encoder
                .write_image(data, width, height, color)
                .map_err(|error| ImageCompressionError::Encode {
                    format,
                    reason: error.to_string(),
                })?;
        }
        ImageFormat::WebP => {
            if !matches!(
                color,
                ExtendedColorType::L8
                    | ExtendedColorType::La8
                    | ExtendedColorType::Rgb8
                    | ExtendedColorType::Rgba8
            ) {
                return Err(ImageCompressionError::UnsupportedColor { format });
            }
            let mut encoder = WebPEncoder::new_lossless(&mut encoded);
            apply_metadata(&mut encoder, metadata)?;
            encoder
                .write_image(data, width, height, color)
                .map_err(|error| ImageCompressionError::Encode {
                    format,
                    reason: error.to_string(),
                })?;
        }
    }
    Ok(encoded)
}

fn validate_candidate(
    output: &Path,
    encoded: &[u8],
    format: ImageFormat,
    width: u32,
    height: u32,
    source_has_alpha: bool,
    metadata: &ImageMetadata,
) -> Result<(), ImageCompressionError> {
    image_conversion::validate_encoded(output, encoded, format, width, height).map_err(
        |error| match error {
            ImageConversionError::OutputValidation { path, reason } => {
                ImageCompressionError::OutputValidation { path, reason }
            }
            other => ImageCompressionError::Image(other),
        },
    )?;

    let mut reader = ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|error| ImageCompressionError::OutputValidation {
            path: output.to_path_buf(),
            reason: error.to_string(),
        })?;
    reader.limits(image_conversion::decode_limits());
    let mut decoder =
        reader
            .into_decoder()
            .map_err(|error| ImageCompressionError::OutputValidation {
                path: output.to_path_buf(),
                reason: error.to_string(),
            })?;

    if source_has_alpha && !decoder.color_type().has_alpha() {
        return Err(ImageCompressionError::OutputValidation {
            path: output.to_path_buf(),
            reason: "transparency was not preserved".to_string(),
        });
    }

    let icc = decoder
        .icc_profile()
        .map_err(|error| ImageCompressionError::OutputValidation {
            path: output.to_path_buf(),
            reason: error.to_string(),
        })?;
    let exif =
        decoder
            .exif_metadata()
            .map_err(|error| ImageCompressionError::OutputValidation {
                path: output.to_path_buf(),
                reason: error.to_string(),
            })?;
    if icc.as_deref() != metadata.icc.as_deref() {
        return Err(ImageCompressionError::OutputValidation {
            path: output.to_path_buf(),
            reason: "ICC metadata did not round-trip".to_string(),
        });
    }
    if exif.as_deref() != metadata.exif.as_deref() {
        return Err(ImageCompressionError::OutputValidation {
            path: output.to_path_buf(),
            reason: "EXIF metadata did not round-trip".to_string(),
        });
    }

    Ok(())
}

fn copy_safely(
    input: &Path,
    output: &Path,
    allow_overwrite: bool,
    progress: &mut dyn Progress,
) -> Result<(), ImageCompressionError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let read_error = |source: io::Error| ImageCompressionError::InputRead {
        path: input.to_path_buf(),
        source,
    };
    let write_error = |source: io::Error| ImageCompressionError::OutputWrite {
        path: output.to_path_buf(),
        source,
    };

    let mut source = fs::File::open(input).map_err(read_error)?;
    let mut temp = NamedTempFile::new_in(parent).map_err(write_error)?;
    let mut buffer = [0u8; COPY_CHUNK_BYTES];

    loop {
        progress.step("copying-image", 0, 0)?;
        let read = source.read(&mut buffer).map_err(read_error)?;
        if read == 0 {
            break;
        }
        temp.write_all(&buffer[..read]).map_err(write_error)?;
    }

    temp.as_file().sync_all().map_err(write_error)?;
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
        Err(error) if !allow_overwrite && error.error.kind() == io::ErrorKind::AlreadyExists => {
            Err(ImageCompressionError::OutputExists {
                path: output.to_path_buf(),
            })
        }
        Err(error) => Err(write_error(error.error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::testing::Recorder;
    use crate::test_images::{
        write_indexed_png, write_jpeg, write_jpeg_image, write_jpeg_with_metadata, write_png,
        write_png_with_metadata, write_webp_with_metadata,
    };
    use image::{GrayAlphaImage, LumaA, Rgb, RgbImage, Rgba, RgbaImage};
    use tempfile::tempdir;

    const ICC: &[u8] = b"quicklet-test-icc-profile";
    const EXIF_ORIENTATION_6: u16 = 6;

    fn metadata_exif() -> Vec<u8> {
        crate::test_images::exif_tiff(EXIF_ORIENTATION_6)
    }

    fn decode(path: &Path) -> DynamicImage {
        image::open(path).unwrap()
    }

    fn assert_png(path: &Path) {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    fn assert_jpeg(path: &Path) {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF]);
    }

    fn assert_lossless_webp(path: &Path) {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        assert!(
            bytes.windows(4).any(|window| window == b"VP8L"),
            "lossless VP8L chunk is missing"
        );
    }

    fn write_optimized_png(path: &Path, image: &RgbaImage) {
        let mut bytes = Vec::new();
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Level(9), FilterType::Adaptive)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgba8,
            )
            .unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn image_bytes(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()
    }

    #[test]
    fn jpeg_quality_reduces_size_and_preserves_source() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.jpg");
        let output = dir.path().join("compressed.jpg");
        let source =
            RgbImage::from_fn(320, 240, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        write_jpeg_image(&input, &source, 95, None);
        let original = image_bytes(&input);

        let outcome =
            compress_image(&input, &output, CompressionOptions { jpeg_quality: 40 }).unwrap();

        assert_eq!(outcome.format, ImageFormat::Jpeg);
        assert_eq!((outcome.width, outcome.height), (320, 240));
        assert!(outcome.output_bytes < outcome.original_bytes);
        assert_eq!(
            outcome.saved_bytes,
            outcome.original_bytes - outcome.output_bytes
        );
        assert_eq!(outcome.status, CompressionStatus::Reduced);
        assert_jpeg(&output);
        assert_eq!(decode(&output).width(), 320);
        assert_eq!(image_bytes(&input), original);
    }

    #[test]
    fn png_compression_is_pixel_identical_and_preserves_alpha() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("alpha.png");
        let output = dir.path().join("alpha-compressed.png");
        let mut source = RgbaImage::new(64, 48);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            *pixel = Rgba([
                (x * 4) as u8,
                (y * 5) as u8,
                120,
                if x < 32 { 0 } else { 255 },
            ]);
        }
        write_png_with_metadata(&input, &source, Some(ICC), Some(&metadata_exif()));

        let outcome = compress_image(&input, &output, CompressionOptions::new()).unwrap();

        assert_eq!(outcome.format, ImageFormat::Png);
        assert_eq!((outcome.width, outcome.height), (64, 48));
        assert!(outcome.had_alpha);
        assert_png(&output);
        assert_eq!(decode(&output).into_rgba8(), source);

        let source_meta = inspect_image(&input).unwrap();
        assert!(source_meta.metadata.has_icc);
        assert!(source_meta.metadata.has_exif);
        let mut decoder = ImageReader::new(Cursor::new(image_bytes(&output)))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.icc_profile().unwrap().as_deref(), Some(ICC));
        assert_eq!(
            decoder.exif_metadata().unwrap().as_deref(),
            Some(metadata_exif().as_slice())
        );
    }

    #[test]
    fn png_preserves_grayscale_and_16_bit_data() {
        let dir = tempdir().unwrap();
        let gray = dir.path().join("gray.png");
        image::GrayImage::from_fn(32, 32, |x, y| image::Luma([(x + y) as u8]))
            .save_with_format(&gray, image::ImageFormat::Png)
            .unwrap();
        let gray_out = dir.path().join("gray-out.png");
        compress_image(&gray, &gray_out, CompressionOptions::new()).unwrap();
        let source = image::open(&gray).unwrap().into_luma8();
        let output = image::open(&gray_out).unwrap().into_luma8();
        assert_eq!(source, output);

        let deep = dir.path().join("deep.png");
        image::ImageBuffer::from_fn(32, 32, |x, y| {
            image::Rgba([(x as u16) * 1000, (y as u16) * 1000, 30000, 65535])
        })
        .save_with_format(&deep, image::ImageFormat::Png)
        .unwrap();
        let deep_out = dir.path().join("deep-out.png");
        compress_image(&deep, &deep_out, CompressionOptions::new()).unwrap();
        let source = image::open(&deep).unwrap().into_rgba16();
        let output = image::open(&deep_out).unwrap().into_rgba16();
        assert_eq!(source, output);
    }

    #[test]
    fn webp_lossless_preserves_alpha_and_pixels() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("alpha.webp");
        let output = dir.path().join("alpha-out.webp");
        let mut source = RgbaImage::new(40, 30);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            *pixel = Rgba([
                (x * 6) as u8,
                (y * 8) as u8,
                200,
                if x < 20 { 0 } else { 255 },
            ]);
        }
        write_webp_with_metadata(&input, &source, Some(ICC), Some(&metadata_exif()));

        let outcome = compress_image(&input, &output, CompressionOptions::new()).unwrap();

        assert_eq!(outcome.format, ImageFormat::WebP);
        assert!(outcome.had_alpha);
        assert_lossless_webp(&output);
        assert_eq!(decode(&output).into_rgba8(), source);
    }

    #[test]
    fn metadata_round_trips_for_all_formats() {
        let dir = tempdir().unwrap();
        let icc = b"quicklet-roundtrip-icc".to_vec();
        let exif = crate::test_images::exif_tiff(6);

        let rgba = RgbaImage::from_fn(8, 8, |x, y| Rgba([(x * 20) as u8, (y * 30) as u8, 90, 255]));
        let rgb = RgbImage::from_fn(8, 8, |x, y| Rgb([(x * 20) as u8, (y * 30) as u8, 90]));

        let png = dir.path().join("meta.png");
        write_png_with_metadata(&png, &rgba, Some(&icc), Some(&exif));
        let png_out = dir.path().join("meta-out.png");
        compress_image(&png, &png_out, CompressionOptions::new()).unwrap();
        assert_metadata(&png_out, &icc, &exif);

        let jpeg = dir.path().join("meta.jpg");
        write_jpeg_with_metadata(&jpeg, &rgb, 85, Some(&icc), Some(&exif));
        let jpeg_out = dir.path().join("meta-out.jpg");
        compress_image(&jpeg, &jpeg_out, CompressionOptions::new()).unwrap();
        assert_metadata(&jpeg_out, &icc, &exif);

        let webp = dir.path().join("meta.webp");
        write_webp_with_metadata(&webp, &rgba, Some(&icc), Some(&exif));
        let webp_out = dir.path().join("meta-out.webp");
        compress_image(&webp, &webp_out, CompressionOptions::new()).unwrap();
        assert_metadata(&webp_out, &icc, &exif);
    }

    fn assert_metadata(path: &Path, icc: &[u8], exif: &[u8]) {
        let mut decoder = ImageReader::new(Cursor::new(image_bytes(path)))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.icc_profile().unwrap().as_deref(), Some(icc));
        assert_eq!(decoder.exif_metadata().unwrap().as_deref(), Some(exif));
    }

    #[test]
    fn orientation_is_preserved_but_pixels_are_not_rotated() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("rotated.jpg");
        let output = dir.path().join("rotated-out.jpg");
        let source = RgbImage::from_fn(20, 10, |x, y| Rgb([(x * 10) as u8, (y * 20) as u8, 90]));
        write_jpeg_image(&input, &source, 85, Some(6));
        let before = inspect_image(&input).unwrap();

        let outcome = compress_image(&input, &output, CompressionOptions::new()).unwrap();

        assert_eq!((outcome.width, outcome.height), (20, 10));
        assert_eq!(outcome.width, before.width);
        assert_eq!(outcome.height, before.height);
        assert_eq!(decode(&output).width(), 20);
        assert_eq!(decode(&output).height(), 10);
        let after = inspect_image(&output).unwrap();
        assert_eq!(after.orientation, 6);
    }

    #[test]
    fn no_savings_copies_exact_original_bytes() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("optimized.png");
        let output = dir.path().join("optimized-out.png");
        let source = RgbaImage::from_fn(64, 64, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 120, 255])
        });
        write_optimized_png(&input, &source);
        let original = image_bytes(&input);

        let outcome = compress_image(&input, &output, CompressionOptions::new()).unwrap();

        assert_eq!(outcome.status, CompressionStatus::NoReduction);
        assert_eq!(outcome.saved_bytes, 0);
        assert_eq!(outcome.reduction_percent, 0.0);
        assert_eq!(image_bytes(&output), original);
    }

    #[test]
    fn cancellation_leaves_no_output_or_temp_files() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.png");
        let output = dir.path().join("photo-out.png");
        write_png(&input, 32, 32, false);
        let original_entries = directory_entry_count(dir.path());

        for cancel_at in 0..6 {
            let result = compress_image_with_progress(
                &input,
                &output,
                CompressionOptions::new(),
                false,
                &mut Recorder::cancelling_at(cancel_at),
            );
            assert!(
                matches!(result, Err(ImageCompressionError::Cancelled)),
                "step {cancel_at}"
            );
            assert_eq!(directory_entry_count(dir.path()), original_entries);
        }
    }

    #[test]
    fn cancellation_during_no_savings_copy_leaves_no_output() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("optimized.png");
        let output = dir.path().join("optimized-out.png");
        let source = RgbaImage::from_fn(64, 64, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 120, 255])
        });
        write_optimized_png(&input, &source);
        let before = directory_entry_count(dir.path());

        let result = compress_image_with_progress(
            &input,
            &output,
            CompressionOptions::new(),
            false,
            &mut Recorder {
                cancel_at: Some(6),
                ..Recorder::default()
            },
        );
        assert!(matches!(result, Err(ImageCompressionError::Cancelled)));
        assert_eq!(directory_entry_count(dir.path()), before);
    }

    fn directory_entry_count(path: &Path) -> usize {
        fs::read_dir(path).unwrap().count()
    }

    #[test]
    fn cancellation_mid_copy_leaves_no_output_or_temp_files() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("large-optimized.png");
        let output = dir.path().join("large-optimized-out.png");
        let mut seed = 0x1234_5678u32;
        let source = RgbaImage::from_fn(256, 256, |_, _| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let [_, b, c, d] = seed.to_le_bytes();
            Rgba([b, c, d, 255])
        });
        write_optimized_png(&input, &source);
        assert!(image_bytes(&input).len() > 3 * COPY_CHUNK_BYTES);
        let before = directory_entry_count(dir.path());

        let result = compress_image_with_progress(
            &input,
            &output,
            CompressionOptions::new(),
            false,
            &mut Recorder {
                cancel_at: Some(8),
                ..Recorder::default()
            },
        );
        assert!(matches!(result, Err(ImageCompressionError::Cancelled)));
        assert_eq!(directory_entry_count(dir.path()), before);
    }

    /// Release-build memory probes for M5. Set the fixture path in the
    /// environment variable, then run the exact ignored test under
    /// `/usr/bin/time -l` to capture peak RSS.
    fn probe_49mp(env_name: &str, quality: u8) {
        let input = std::path::PathBuf::from(
            std::env::var(env_name).unwrap_or_else(|_| panic!("{env_name} is not set")),
        );
        let dir = tempdir().unwrap();
        let extension = input
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("img");
        let output = dir.path().join(format!("probe-out.{extension}"));
        let input_size = fs::metadata(&input).unwrap().len();
        let started = std::time::Instant::now();

        let outcome = compress_image(
            &input,
            &output,
            CompressionOptions {
                jpeg_quality: quality,
            },
        )
        .unwrap();
        let elapsed = started.elapsed();

        println!(
            "input={} input_bytes={} format={:?} dimensions={}x{} quality={} output_bytes={} saved_bytes={} reduction={:.2}% status={:?} duration={:.3}s",
            input.display(),
            input_size,
            outcome.format,
            outcome.width,
            outcome.height,
            quality,
            outcome.output_bytes,
            outcome.saved_bytes,
            outcome.reduction_percent,
            outcome.status,
            elapsed.as_secs_f64(),
        );
    }

    #[test]
    #[ignore]
    fn probe_peak_memory_jpeg_49mp() {
        probe_49mp("QUICKLET_M5_JPEG", 85);
    }

    #[test]
    #[ignore]
    fn probe_peak_memory_png_49mp() {
        probe_49mp("QUICKLET_M5_PNG", 85);
    }

    #[test]
    #[ignore]
    fn probe_peak_memory_webp_49mp() {
        probe_49mp("QUICKLET_M5_WEBP", 85);
    }

    #[test]
    fn candidate_validation_rejects_bad_dimensions_and_metadata() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("meta.png");
        let output = dir.path().join("out.png");
        let rgba = RgbaImage::from_fn(8, 8, |x, y| Rgba([x as u8, y as u8, 10, 255]));
        write_png_with_metadata(&input, &rgba, Some(ICC), Some(&metadata_exif()));
        let (mut decoder, _) =
            image_conversion::open_decoder(&input, image_bytes(&input)).expect("decoder");
        let metadata = capture_metadata(&mut decoder, &input).unwrap();
        let encoded = image_bytes(&input);

        assert!(
            validate_candidate(&output, &encoded, ImageFormat::Png, 9, 8, true, &metadata,)
                .is_err()
        );

        let mut wrong_metadata = metadata.clone();
        wrong_metadata.icc = Some(b"different".to_vec());
        assert!(
            validate_candidate(
                &output,
                &encoded,
                ImageFormat::Png,
                8,
                8,
                true,
                &wrong_metadata,
            )
            .is_err()
        );
    }

    #[test]
    fn jpeg_quality_levels_preserve_dimensions_format_and_metadata() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("quality.jpg");
        let source = RgbImage::from_fn(96, 64, |x, y| Rgb([(x * 2) as u8, (y * 3) as u8, 160]));
        write_jpeg_with_metadata(&input, &source, 95, Some(ICC), Some(&metadata_exif()));

        for quality in [85u8, 100, 75, 50] {
            let output = dir.path().join(format!("quality-{quality}.jpg"));
            let outcome = compress_image(
                &input,
                &output,
                CompressionOptions {
                    jpeg_quality: quality,
                },
            )
            .unwrap();

            assert_eq!(outcome.format, ImageFormat::Jpeg);
            assert_eq!((outcome.width, outcome.height), (96, 64));
            assert_jpeg(&output);
            let decoded = decode(&output);
            assert_eq!((decoded.width(), decoded.height()), (96, 64));
            assert_metadata(&output, ICC, &metadata_exif());
            assert_eq!(inspect_image(&output).unwrap().orientation, 6);
        }
    }

    #[test]
    fn png_preserves_rgb_grayscale_grayscale_alpha_and_indexed_pixels() {
        let dir = tempdir().unwrap();

        let rgb = dir.path().join("rgb.png");
        let rgb_source = RgbImage::from_fn(32, 24, |x, y| Rgb([(x * 7) as u8, (y * 9) as u8, 90]));
        rgb_source
            .save_with_format(&rgb, image::ImageFormat::Png)
            .unwrap();
        let rgb_out = dir.path().join("rgb-out.png");
        compress_image(&rgb, &rgb_out, CompressionOptions::new()).unwrap();
        assert_eq!(decode(&rgb_out).into_rgb8(), rgb_source);

        let gray_alpha = dir.path().join("gray-alpha.png");
        let gray_alpha_source = GrayAlphaImage::from_fn(32, 24, |x, _y| {
            LumaA([(x * 6) as u8, if x < 16 { 0 } else { 255 }])
        });
        gray_alpha_source
            .save_with_format(&gray_alpha, image::ImageFormat::Png)
            .unwrap();
        let gray_alpha_out = dir.path().join("gray-alpha-out.png");
        compress_image(&gray_alpha, &gray_alpha_out, CompressionOptions::new()).unwrap();
        assert_eq!(
            decode(&gray_alpha_out).into_luma_alpha8(),
            gray_alpha_source
        );

        let indexed = dir.path().join("indexed.png");
        let palette = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]];
        let indices: Vec<u8> = (0..32 * 24)
            .map(|index| (index % palette.len() as u32) as u8)
            .collect();
        write_indexed_png(&indexed, 32, 24, &palette, &indices);
        let indexed_out = dir.path().join("indexed-out.png");
        compress_image(&indexed, &indexed_out, CompressionOptions::new()).unwrap();
        let expected = RgbImage::from_fn(32, 24, |x, y| {
            let index = ((y * 32 + x) as usize) % palette.len();
            let [r, g, b] = palette[index];
            Rgb([r, g, b])
        });
        assert_eq!(decode(&indexed_out).into_rgb8(), expected);
    }

    #[test]
    fn already_optimized_webp_is_no_savings_and_keeps_exact_bytes() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("optimized.webp");
        let output = dir.path().join("optimized-out.webp");
        let source = RgbaImage::from_fn(64, 64, |x, y| {
            Rgba([(x * 4) as u8, (y * 4) as u8, 120, 255])
        });
        write_webp_with_metadata(&input, &source, Some(ICC), Some(&metadata_exif()));
        let original = image_bytes(&input);

        let outcome = compress_image(&input, &output, CompressionOptions::new()).unwrap();

        assert_eq!(outcome.status, CompressionStatus::NoReduction);
        assert_eq!(outcome.saved_bytes, 0);
        assert_eq!(outcome.reduction_percent, 0.0);
        assert_eq!(image_bytes(&output), original);
        assert_metadata(&output, ICC, &metadata_exif());
    }

    #[test]
    fn overwrite_replaces_an_existing_destination_when_allowed() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.png");
        let output = dir.path().join("photo-out.png");
        write_png(&input, 64, 64, false);
        fs::write(&output, b"existing").unwrap();

        let outcome =
            compress_image_with_overwrite(&input, &output, CompressionOptions::new()).unwrap();

        assert_eq!(outcome.status, CompressionStatus::Reduced);
        assert_ne!(image_bytes(&output), b"existing");
        assert_png(&output);
    }

    #[test]
    fn metadata_cap_is_enforced() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("large-metadata.png");
        let output = dir.path().join("out.png");
        let rgba = RgbaImage::from_fn(2, 2, |_, _| Rgba([1, 2, 3, 255]));
        let icc = vec![0u8; MAX_PRESERVED_METADATA_BYTES + 1];
        write_png_with_metadata(&input, &rgba, Some(&icc), None);

        let result = compress_image(&input, &output, CompressionOptions::new());

        assert!(matches!(
            result,
            Err(ImageCompressionError::MetadataTooLarge { .. })
        ));
        assert!(!output.exists());
    }

    #[test]
    fn rejects_invalid_quality_and_unsupported_inputs() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.jpg");
        let output = dir.path().join("out.jpg");
        write_jpeg(&input, 8, 8, None);

        let result = compress_image(&input, &output, CompressionOptions { jpeg_quality: 0 });
        assert!(matches!(
            result,
            Err(ImageCompressionError::InvalidQuality { .. })
        ));

        let missing = dir.path().join("missing.png");
        assert!(matches!(
            compress_image(&missing, &output, CompressionOptions::new()),
            Err(ImageCompressionError::Image(
                ImageConversionError::InputNotFound { .. }
            ))
        ));

        let text = dir.path().join("text.png");
        fs::write(&text, b"not an image").unwrap();
        assert!(matches!(
            compress_image(&text, &output, CompressionOptions::new()),
            Err(ImageCompressionError::Image(
                ImageConversionError::UnsupportedFormat { .. }
            ))
        ));
    }

    #[test]
    fn rejects_animated_truncated_and_oversized_inputs() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("out.png");

        let truncated = dir.path().join("truncated.png");
        let good = dir.path().join("good.png");
        write_png(&good, 64, 64, false);
        let bytes = image_bytes(&good);
        fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            compress_image(&truncated, &output, CompressionOptions::new()),
            Err(ImageCompressionError::Image(
                ImageConversionError::InvalidImage { .. }
            ))
        ));
        assert!(!output.exists());

        let animated = dir.path().join("animated.webp");
        let mut bytes = b"RIFF\x1e\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&[0x02, 0, 0, 0, 7, 0, 0, 7, 0, 0]);
        bytes.extend_from_slice(b"ANIM\x06\x00\x00\x00\0\0\0\0\0\0");
        fs::write(&animated, bytes).unwrap();
        assert!(matches!(
            compress_image(&animated, &output, CompressionOptions::new()),
            Err(ImageCompressionError::Image(
                ImageConversionError::AnimatedImage { .. }
            ))
        ));

        let oversized = dir.path().join("oversized.png");
        let file = fs::File::create(&oversized).unwrap();
        file.set_len(crate::image_conversion::MAX_INPUT_BYTES + 1)
            .unwrap();
        assert!(matches!(
            compress_image(&oversized, &output, CompressionOptions::new()),
            Err(ImageCompressionError::Image(
                ImageConversionError::InputTooLarge { .. }
            ))
        ));
    }

    #[test]
    fn records_operation_stage_order() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.png");
        let output = dir.path().join("photo-out.png");
        write_png(&input, 64, 64, false);
        let mut recorder = Recorder::default();

        compress_image_with_progress(
            &input,
            &output,
            CompressionOptions::new(),
            false,
            &mut recorder,
        )
        .unwrap();

        let stages: Vec<_> = recorder.steps.iter().map(|(stage, _, _)| *stage).collect();
        assert_eq!(
            stages,
            [
                "reading-image",
                "reading-metadata",
                "decoding",
                "encoding",
                "validating-image",
                "saving-image"
            ]
        );
    }

    #[test]
    fn no_savings_uses_the_copying_stage() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("optimized.png");
        let output = dir.path().join("optimized-out.png");
        let source = RgbaImage::from_fn(64, 64, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 120, 255])
        });
        write_optimized_png(&input, &source);
        let mut recorder = Recorder::default();

        let outcome = compress_image_with_progress(
            &input,
            &output,
            CompressionOptions::new(),
            false,
            &mut recorder,
        )
        .unwrap();

        assert_eq!(outcome.status, CompressionStatus::NoReduction);
        let stages: Vec<_> = recorder.steps.iter().map(|(stage, _, _)| *stage).collect();
        assert_eq!(
            &stages[..6],
            [
                "reading-image",
                "reading-metadata",
                "decoding",
                "encoding",
                "validating-image",
                "saving-image"
            ]
        );
        assert!(stages[6..].iter().all(|stage| *stage == "copying-image"));
        assert!(!stages.is_empty());
    }

    #[test]
    fn cancellation_can_be_followed_by_a_successful_retry() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.png");
        let output = dir.path().join("photo-out.png");
        write_png(&input, 64, 64, false);

        let cancelled = compress_image_with_progress(
            &input,
            &output,
            CompressionOptions::new(),
            false,
            &mut Recorder::cancelling_at(3),
        );
        assert!(matches!(cancelled, Err(ImageCompressionError::Cancelled)));
        assert!(!output.exists());

        let outcome = compress_image(&input, &output, CompressionOptions::new()).unwrap();
        assert_eq!(outcome.status, CompressionStatus::Reduced);
        assert!(output.exists());
    }

    #[test]
    fn existing_output_is_protected_without_confirmation() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("photo.png");
        let output = dir.path().join("photo-out.png");
        write_png(&input, 16, 16, false);
        fs::write(&output, b"existing").unwrap();

        let result = compress_image(&input, &output, CompressionOptions::new());
        assert!(matches!(
            result,
            Err(ImageCompressionError::Image(
                ImageConversionError::OutputExists { .. }
            ))
        ));
        assert_eq!(fs::read(&output).unwrap(), b"existing");
    }
}
