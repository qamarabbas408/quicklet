//! Converts one static PNG, JPEG or WebP image to another of those formats.
//!
//! The source format is detected from the file content, never from the
//! extension. Pixels keep their dimensions: nothing is resized or cropped.
//!
//! # Metadata policy
//!
//! EXIF orientation is applied to the pixels exactly once, and the output
//! carries no EXIF at all, so a viewer cannot apply it a second time. ICC
//! colour profiles are not carried over either; a wide-gamut source may look
//! slightly different after conversion. No other metadata survives.
//!
//! # Memory
//!
//! The decoder is capped at [`MAX_DECODE_ALLOC`], which holds a
//! [`MAX_PIXELS`] image at 8 bits per channel but rejects one at 16 bits. The
//! source bytes are released before the pixels are transformed. Rotations by
//! 90 or 270 degrees copy the image once (flips happen in place), and
//! flattening alpha for JPEG copies it once more at 3 bytes per pixel, so the
//! peak for the largest allowed image is roughly 2 decoded copies plus the
//! encoded output.

use std::fmt;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{PngDecoder, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader, Limits};
use tempfile::NamedTempFile;

use crate::pdf::is_animated_webp;
use crate::progress::{self, Progress};

pub const MAX_INPUT_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_DIMENSION: u32 = 16_384;
pub const MAX_PIXELS: u64 = 50_000_000;
/// Decoder allocation cap: 50 MP at RGBA8 is 200 MiB.
pub const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
pub const DEFAULT_JPEG_QUALITY: u8 = 85;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
}

impl ImageFormat {
    pub const ALL: [ImageFormat; 3] = [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "png" => Some(Self::Png),
            "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }

    fn from_detected(format: image::ImageFormat) -> Option<Self> {
        match format {
            image::ImageFormat::Png => Some(Self::Png),
            image::ImageFormat::Jpeg => Some(Self::Jpeg),
            image::ImageFormat::WebP => Some(Self::WebP),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::WebP => "webp",
        }
    }

    /// Shown to the user. WebP output is always lossless.
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP (Lossless)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }

    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Jpeg)
    }

    pub fn supports_alpha(self) -> bool {
        !matches!(self, Self::Jpeg)
    }

    /// Destinations offered for a source of this format: every other format.
    pub fn destinations(self) -> Vec<ImageFormat> {
        Self::ALL
            .into_iter()
            .filter(|format| *format != self)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversionOptions {
    pub format: ImageFormat,
    /// 1–100. Only used for JPEG output.
    pub jpeg_quality: u8,
    /// Fill for transparent areas when the output cannot hold alpha.
    pub background: [u8; 3],
}

impl ConversionOptions {
    pub fn new(format: ImageFormat) -> Self {
        Self {
            format,
            jpeg_quality: DEFAULT_JPEG_QUALITY,
            background: [255, 255, 255],
        }
    }
}

/// Parses `#rrggbb` into a colour.
pub fn parse_background(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSource {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub has_alpha: bool,
    /// EXIF orientation 1–8 as stored in the file (1 when absent).
    pub orientation: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversionOutcome {
    pub format: ImageFormat,
    /// Dimensions after orientation was applied.
    pub width: u32,
    pub height: u32,
    pub bytes_written: u64,
    pub had_alpha: bool,
    /// Alpha was flattened onto the background colour.
    pub flattened: bool,
}

#[derive(Debug)]
pub enum ImageConversionError {
    InputNotFound {
        path: PathBuf,
    },
    InputRead {
        path: PathBuf,
        source: io::Error,
    },
    InputTooLarge {
        path: PathBuf,
        bytes: u64,
        max: u64,
    },
    UnsupportedFormat {
        path: PathBuf,
        detected: Option<String>,
    },
    AnimatedImage {
        path: PathBuf,
    },
    DimensionTooLarge {
        path: PathBuf,
        width: u32,
        height: u32,
        max: u32,
    },
    TooManyPixels {
        path: PathBuf,
        width: u32,
        height: u32,
        max: u64,
    },
    DecodeLimit {
        path: PathBuf,
    },
    InvalidImage {
        path: PathBuf,
        reason: String,
    },
    SameFormat {
        format: ImageFormat,
    },
    InvalidQuality {
        quality: u8,
    },
    OutputExists {
        path: PathBuf,
    },
    OutputDirectoryInvalid {
        path: PathBuf,
    },
    OutputMatchesInput {
        path: PathBuf,
    },
    OutputWrite {
        path: PathBuf,
        source: io::Error,
    },
    OutputValidation {
        path: PathBuf,
        reason: String,
    },
    Cancelled,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl fmt::Display for ImageConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputNotFound { path } => {
                write!(formatter, "Image not found: {}", file_name(path))
            }
            Self::InputRead { path, .. } => {
                write!(formatter, "Could not read {}.", file_name(path))
            }
            Self::InputTooLarge { path, bytes, max } => write!(
                formatter,
                "{} is too large ({:.0} MB). The limit is {:.0} MB.",
                file_name(path),
                *bytes as f64 / (1024.0 * 1024.0),
                *max as f64 / (1024.0 * 1024.0)
            ),
            Self::UnsupportedFormat { path, detected } => match detected {
                Some(detected) => write!(
                    formatter,
                    "{} is a {detected} image. Only PNG, JPEG, and WebP are supported.",
                    file_name(path)
                ),
                None => write!(
                    formatter,
                    "{} is not a PNG, JPEG, or WebP image.",
                    file_name(path)
                ),
            },
            Self::AnimatedImage { path } => write!(
                formatter,
                "{} is animated. Only still images are supported.",
                file_name(path)
            ),
            Self::DimensionTooLarge {
                path,
                width,
                height,
                max,
            } => write!(
                formatter,
                "{} is {width}×{height} pixels. Each side must be at most {max} pixels.",
                file_name(path)
            ),
            Self::TooManyPixels {
                path,
                width,
                height,
                max,
            } => write!(
                formatter,
                "{} is {width}×{height} pixels. The limit is {} megapixels.",
                file_name(path),
                max / 1_000_000
            ),
            Self::DecodeLimit { path } => write!(
                formatter,
                "{} needs more memory to decode than Quicklet allows.",
                file_name(path)
            ),
            Self::InvalidImage { path, .. } => write!(
                formatter,
                "Could not decode {}. The file may be damaged.",
                file_name(path)
            ),
            Self::SameFormat { format } => write!(
                formatter,
                "This image is already {}. Choose a different output format.",
                format.label()
            ),
            Self::InvalidQuality { quality } => {
                write!(
                    formatter,
                    "Quality must be between 1 and 100 (got {quality})."
                )
            }
            Self::OutputExists { path } => write!(
                formatter,
                "A file named \"{}\" already exists. Choose a different name or folder.",
                file_name(path)
            ),
            Self::OutputDirectoryInvalid { path } => write!(
                formatter,
                "Output folder does not exist: {}",
                path.display()
            ),
            Self::OutputMatchesInput { .. } => {
                write!(formatter, "Choose an output that is not the source image.")
            }
            Self::OutputWrite { path, .. } => {
                write!(formatter, "Could not write {}.", file_name(path))
            }
            Self::OutputValidation { path, .. } => write!(
                formatter,
                "The converted image could not be verified, so {} was not written.",
                file_name(path)
            ),
            Self::Cancelled => write!(formatter, "Conversion cancelled."),
        }
    }
}

impl std::error::Error for ImageConversionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InputRead { source, .. } | Self::OutputWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<progress::Cancelled> for ImageConversionError {
    fn from(_: progress::Cancelled) -> Self {
        Self::Cancelled
    }
}

impl progress::OperationError for ImageConversionError {
    fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// Reads the header only; no pixels are decoded.
pub fn inspect_image(input: &Path) -> Result<ImageSource, ImageConversionError> {
    let bytes = read_input(input)?;
    let (_, source) = open_decoder(input, bytes)?;
    Ok(source)
}

/// Converts `input` and writes the result to `output`. Reports `reading-image`,
/// `decoding`, `orienting`, `encoding` and `saving-image`, and stops before
/// writing if `progress` cancels.
pub fn convert_image_with_progress(
    input: &Path,
    output: &Path,
    options: ConversionOptions,
    allow_overwrite: bool,
    progress: &mut dyn Progress,
) -> Result<ConversionOutcome, ImageConversionError> {
    if options.format.is_lossy() && !(1..=100).contains(&options.jpeg_quality) {
        return Err(ImageConversionError::InvalidQuality {
            quality: options.jpeg_quality,
        });
    }

    progress.step("reading-image", 0, 0)?;
    let bytes = read_input(input)?;
    let (decoder, source) = open_decoder(input, bytes)?;
    if source.format == options.format {
        return Err(ImageConversionError::SameFormat {
            format: source.format,
        });
    }
    validate_output(input, output, allow_overwrite)?;

    progress.step("decoding", 0, 0)?;
    // Consuming the decoder releases the source bytes before any copies are made.
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|error| decode_error(input, error))?;
    image = if source.has_alpha {
        DynamicImage::ImageRgba8(image.into_rgba8())
    } else {
        DynamicImage::ImageRgb8(image.into_rgb8())
    };

    progress.step("orienting", 0, 0)?;
    if let Some(orientation) = Orientation::from_exif(source.orientation) {
        image.apply_orientation(orientation);
    }

    progress.step("encoding", 0, 0)?;
    let flattened = source.has_alpha && !options.format.supports_alpha();
    let encoded = encode(&image, options, flattened)?;
    let (width, height) = (image.width(), image.height());
    drop(image);
    validate_encoded(output, &encoded, options.format, width, height)?;

    progress.step("saving-image", 0, 0)?;
    write_safely(output, &encoded, allow_overwrite)?;

    Ok(ConversionOutcome {
        format: options.format,
        width,
        height,
        bytes_written: encoded.len() as u64,
        had_alpha: source.has_alpha,
        flattened,
    })
}

fn read_input(path: &Path) -> Result<Vec<u8>, ImageConversionError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ImageConversionError::InputNotFound {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(ImageConversionError::InputRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.is_dir() {
        return Err(ImageConversionError::UnsupportedFormat {
            path: path.to_path_buf(),
            detected: None,
        });
    }
    if metadata.len() > MAX_INPUT_BYTES {
        return Err(ImageConversionError::InputTooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            max: MAX_INPUT_BYTES,
        });
    }
    fs::read(path).map_err(|source| ImageConversionError::InputRead {
        path: path.to_path_buf(),
        source,
    })
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}

fn decode_error(path: &Path, error: image::ImageError) -> ImageConversionError {
    match error {
        image::ImageError::Limits(_) => ImageConversionError::DecodeLimit {
            path: path.to_path_buf(),
        },
        error => ImageConversionError::InvalidImage {
            path: path.to_path_buf(),
            reason: error.to_string(),
        },
    }
}

/// Detects the format from `bytes`, checks the limits, and returns a decoder
/// positioned to decode the pixels.
fn open_decoder(
    path: &Path,
    bytes: Vec<u8>,
) -> Result<(impl ImageDecoder + 'static, ImageSource), ImageConversionError> {
    let size = bytes.len() as u64;
    let detected = image::guess_format(&bytes).ok();
    let format = detected
        .and_then(ImageFormat::from_detected)
        .ok_or_else(|| ImageConversionError::UnsupportedFormat {
            path: path.to_path_buf(),
            detected: detected.map(|format| format!("{format:?}")),
        })?;

    let animated = match format {
        ImageFormat::WebP => is_animated_webp(&bytes),
        ImageFormat::Png => PngDecoder::with_limits(Cursor::new(&bytes), decode_limits())
            .and_then(|decoder| decoder.is_apng())
            .map_err(|error| decode_error(path, error))?,
        ImageFormat::Jpeg => false,
    };
    if animated {
        return Err(ImageConversionError::AnimatedImage {
            path: path.to_path_buf(),
        });
    }

    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|source| ImageConversionError::InputRead {
            path: path.to_path_buf(),
            source,
        })?;
    reader.limits(decode_limits());
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| decode_error(path, error))?;

    let (width, height) = decoder.dimensions();
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(ImageConversionError::DimensionTooLarge {
            path: path.to_path_buf(),
            width,
            height,
            max: MAX_DIMENSION,
        });
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageConversionError::TooManyPixels {
            path: path.to_path_buf(),
            width,
            height,
            max: MAX_PIXELS,
        });
    }

    // `ImageReader::limits` is not enforced for every decode path, so the
    // decoded size is bounded here from the header before any pixel is read.
    if decoder.total_bytes() > MAX_DECODE_ALLOC {
        return Err(ImageConversionError::DecodeLimit {
            path: path.to_path_buf(),
        });
    }

    let has_alpha = decoder.color_type().has_alpha();
    let orientation = match decoder.orientation() {
        Ok(orientation) => orientation.to_exif(),
        // Malformed EXIF is not a reason to refuse the image.
        Err(_) => 1,
    };

    Ok((
        decoder,
        ImageSource {
            format,
            width,
            height,
            bytes: size,
            has_alpha,
            orientation,
        },
    ))
}

fn encode(
    image: &DynamicImage,
    options: ConversionOptions,
    flatten: bool,
) -> Result<Vec<u8>, ImageConversionError> {
    let (width, height) = (image.width(), image.height());
    let (data, color) = match image {
        DynamicImage::ImageRgba8(rgba) => (rgba.as_raw().as_slice(), ExtendedColorType::Rgba8),
        DynamicImage::ImageRgb8(rgb) => (rgb.as_raw().as_slice(), ExtendedColorType::Rgb8),
        _ => unreachable!("image is converted to 8-bit RGB or RGBA before encoding"),
    };

    let mut encoded = Vec::new();
    let result = match options.format {
        ImageFormat::Png => PngEncoder::new(&mut encoded).write_image(data, width, height, color),
        ImageFormat::WebP => {
            WebPEncoder::new_lossless(&mut encoded).write_image(data, width, height, color)
        }
        ImageFormat::Jpeg => {
            let encoder = JpegEncoder::new_with_quality(&mut encoded, options.jpeg_quality);
            if flatten {
                let flat = flatten_alpha(data, options.background);
                encoder.write_image(&flat, width, height, ExtendedColorType::Rgb8)
            } else {
                encoder.write_image(data, width, height, color)
            }
        }
    };
    result.map_err(|error| ImageConversionError::OutputValidation {
        path: PathBuf::new(),
        reason: error.to_string(),
    })?;
    Ok(encoded)
}

/// Composites RGBA8 pixels onto `background`, returning RGB8.
fn flatten_alpha(rgba: &[u8], background: [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for pixel in rgba.chunks_exact(4) {
        let alpha = u32::from(pixel[3]);
        for channel in 0..3 {
            let value = u32::from(pixel[channel]) * alpha
                + u32::from(background[channel]) * (255 - alpha)
                + 127;
            rgb.push((value / 255) as u8);
        }
    }
    rgb
}

/// Decodes the header of the encoded output and checks it is what was asked for.
fn validate_encoded(
    output: &Path,
    encoded: &[u8],
    format: ImageFormat,
    width: u32,
    height: u32,
) -> Result<(), ImageConversionError> {
    let validation = |reason: String| ImageConversionError::OutputValidation {
        path: output.to_path_buf(),
        reason,
    };
    let detected = image::guess_format(encoded)
        .ok()
        .and_then(ImageFormat::from_detected);
    if detected != Some(format) {
        return Err(validation(format!(
            "expected {format:?}, found {detected:?}"
        )));
    }
    let mut reader = ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|error| validation(error.to_string()))?;
    reader.limits(decode_limits());
    let dimensions = reader
        .into_dimensions()
        .map_err(|error| validation(error.to_string()))?;
    if dimensions != (width, height) {
        return Err(validation(format!(
            "expected {width}×{height}, found {}×{}",
            dimensions.0, dimensions.1
        )));
    }
    Ok(())
}

fn validate_output(
    input: &Path,
    output: &Path,
    allow_overwrite: bool,
) -> Result<(), ImageConversionError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(ImageConversionError::OutputDirectoryInvalid {
            path: parent.to_path_buf(),
        });
    }

    if output.exists() {
        let same_file = match (fs::canonicalize(input), fs::canonicalize(output)) {
            (Ok(input), Ok(output)) => input == output,
            _ => false,
        };
        if same_file {
            return Err(ImageConversionError::OutputMatchesInput {
                path: output.to_path_buf(),
            });
        }
        if output.is_dir() || !allow_overwrite {
            return Err(ImageConversionError::OutputExists {
                path: output.to_path_buf(),
            });
        }
    }
    Ok(())
}

fn write_safely(
    output: &Path,
    bytes: &[u8],
    allow_overwrite: bool,
) -> Result<(), ImageConversionError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let write_error = |source: io::Error| ImageConversionError::OutputWrite {
        path: output.to_path_buf(),
        source,
    };

    let mut temp = NamedTempFile::new_in(parent).map_err(write_error)?;
    temp.write_all(bytes).map_err(write_error)?;
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
            Err(ImageConversionError::OutputExists {
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
        write_jpeg, write_jpeg_image, write_png, write_png_image, write_webp, write_webp_image,
    };
    use image::{Rgb, RgbImage, Rgba, RgbaImage};
    use tempfile::tempdir;

    fn convert(
        input: &Path,
        output: &Path,
        options: ConversionOptions,
    ) -> Result<ConversionOutcome, ImageConversionError> {
        convert_image_with_progress(input, output, options, false, &mut ())
    }

    fn decode(path: &Path) -> DynamicImage {
        image::open(path).unwrap()
    }

    fn signature(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()[..16].to_vec()
    }

    fn assert_png(path: &Path) {
        assert_eq!(&signature(path)[..8], b"\x89PNG\r\n\x1a\n");
    }

    fn assert_jpeg(path: &Path) {
        assert_eq!(&signature(path)[..3], &[0xFF, 0xD8, 0xFF]);
    }

    fn assert_lossless_webp(path: &Path) {
        let bytes = signature(path);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(&bytes[12..16], b"VP8L");
    }

    /// A 3×2 image where every pixel has a distinct colour.
    fn pattern() -> RgbaImage {
        let mut image = RgbaImage::new(3, 2);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([(x * 80 + 20) as u8, (y * 120 + 30) as u8, 200, 255]);
        }
        image
    }

    /// Where EXIF orientation `orientation` displays the stored pixel (x, y),
    /// written out independently of the image crate.
    fn displayed_position(orientation: u16, (w, h): (u32, u32), x: u32, y: u32) -> (u32, u32) {
        match orientation {
            1 => (x, y),
            2 => (w - 1 - x, y),
            3 => (w - 1 - x, h - 1 - y),
            4 => (x, h - 1 - y),
            5 => (y, x),
            6 => (h - 1 - y, x),
            7 => (h - 1 - y, w - 1 - x),
            8 => (y, w - 1 - x),
            _ => unreachable!(),
        }
    }

    fn displayed_size(orientation: u16, (w, h): (u32, u32)) -> (u32, u32) {
        if orientation >= 5 { (h, w) } else { (w, h) }
    }

    #[test]
    fn converts_every_supported_pair_with_matching_signatures() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("source.png");
        let jpeg = dir.path().join("source.jpg");
        let webp = dir.path().join("source.webp");
        write_png(&png, 40, 30, false);
        write_jpeg(&jpeg, 40, 30, None);
        write_webp(&webp, 40, 30);

        let cases = [
            (&png, ImageFormat::Jpeg, "png-to.jpg"),
            (&png, ImageFormat::WebP, "png-to.webp"),
            (&jpeg, ImageFormat::Png, "jpeg-to.png"),
            (&jpeg, ImageFormat::WebP, "jpeg-to.webp"),
            (&webp, ImageFormat::Png, "webp-to.png"),
            (&webp, ImageFormat::Jpeg, "webp-to.jpg"),
        ];
        for (input, format, name) in cases {
            let output = dir.path().join(name);
            let outcome = convert(input, &output, ConversionOptions::new(format)).unwrap();

            assert_eq!(outcome.format, format, "{name}");
            assert_eq!((outcome.width, outcome.height), (40, 30), "{name}");
            assert_eq!(outcome.bytes_written, fs::metadata(&output).unwrap().len());
            match format {
                ImageFormat::Png => assert_png(&output),
                ImageFormat::Jpeg => assert_jpeg(&output),
                ImageFormat::WebP => assert_lossless_webp(&output),
            }
            let decoded = decode(&output);
            assert_eq!((decoded.width(), decoded.height()), (40, 30), "{name}");
        }
    }

    #[test]
    fn rejects_same_format_conversion() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("source.png");
        write_png(&png, 8, 8, false);
        let output = dir.path().join("copy.png");

        let result = convert(&png, &output, ConversionOptions::new(ImageFormat::Png));

        assert!(matches!(
            result,
            Err(ImageConversionError::SameFormat {
                format: ImageFormat::Png
            })
        ));
        assert!(!output.exists());
    }

    #[test]
    fn preserves_alpha_for_png_and_webp() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("alpha.png");
        write_png(&png, 20, 10, true);

        let webp = dir.path().join("alpha.webp");
        let outcome = convert(&png, &webp, ConversionOptions::new(ImageFormat::WebP)).unwrap();
        assert!(outcome.had_alpha);
        assert!(!outcome.flattened);
        let decoded = decode(&webp).into_rgba8();
        assert_eq!(decoded.get_pixel(0, 0)[3], 0);
        assert_eq!(decoded.get_pixel(19, 9)[3], 255);

        let back = dir.path().join("alpha-back.png");
        convert(&webp, &back, ConversionOptions::new(ImageFormat::Png)).unwrap();
        assert_eq!(decode(&back).into_rgba8().get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn flattens_alpha_onto_background_for_jpeg() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("alpha.png");
        let mut image = RgbaImage::new(16, 16);
        for (x, _, pixel) in image.enumerate_pixels_mut() {
            *pixel = if x < 8 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([0, 0, 0, 255])
            };
        }
        image.save(&png).unwrap();

        let white = dir.path().join("white.jpg");
        let outcome = convert(&png, &white, ConversionOptions::new(ImageFormat::Jpeg)).unwrap();
        assert!(outcome.had_alpha);
        assert!(outcome.flattened);
        let decoded = decode(&white).into_rgb8();
        assert!(
            decoded.get_pixel(2, 8)[0] > 240,
            "transparent area is white"
        );
        assert!(decoded.get_pixel(13, 8)[0] < 15, "opaque area is kept");

        let black = dir.path().join("black.jpg");
        let options = ConversionOptions {
            background: [0, 0, 0],
            ..ConversionOptions::new(ImageFormat::Jpeg)
        };
        convert(&png, &black, options).unwrap();
        assert!(decode(&black).into_rgb8().get_pixel(2, 8)[0] < 15);
    }

    #[test]
    fn flatten_alpha_blends_partial_transparency() {
        let rgb = flatten_alpha(&[200, 100, 0, 128, 10, 20, 30, 255], [255, 255, 255]);
        assert_eq!(rgb, vec![227, 177, 127, 10, 20, 30]);
    }

    #[test]
    fn applies_every_exif_orientation_from_webp_exactly_once() {
        let dir = tempdir().unwrap();
        let source = pattern();

        for orientation in 1..=8u16 {
            let input = dir.path().join(format!("o{orientation}.webp"));
            write_webp_image(&input, &source, Some(orientation));
            assert_eq!(
                inspect_image(&input).unwrap().orientation,
                orientation as u8
            );

            let output = dir.path().join(format!("o{orientation}.png"));
            let outcome =
                convert(&input, &output, ConversionOptions::new(ImageFormat::Png)).unwrap();
            let expected_size = displayed_size(orientation, (3, 2));
            assert_eq!(
                (outcome.width, outcome.height),
                expected_size,
                "orientation {orientation}"
            );

            let decoded = decode(&output).into_rgba8();
            assert_eq!((decoded.width(), decoded.height()), expected_size);
            for (x, y, pixel) in source.enumerate_pixels() {
                let (dx, dy) = displayed_position(orientation, (3, 2), x, y);
                assert_eq!(
                    decoded.get_pixel(dx, dy),
                    pixel,
                    "orientation {orientation} at ({x}, {y})"
                );
            }
            // The output has no EXIF, so inspecting it reports no rotation.
            assert_eq!(inspect_image(&output).unwrap().orientation, 1);
        }
    }

    #[test]
    fn applies_every_exif_orientation_from_png_exactly_once() {
        let dir = tempdir().unwrap();
        let source = pattern();

        for orientation in 1..=8u16 {
            let input = dir.path().join(format!("o{orientation}.png"));
            write_png_image(&input, &source, Some(orientation));
            assert_eq!(
                inspect_image(&input).unwrap().orientation,
                orientation as u8
            );

            let output = dir.path().join(format!("o{orientation}.webp"));
            let outcome =
                convert(&input, &output, ConversionOptions::new(ImageFormat::WebP)).unwrap();
            let expected_size = displayed_size(orientation, (3, 2));
            assert_eq!(
                (outcome.width, outcome.height),
                expected_size,
                "orientation {orientation}"
            );

            let decoded = decode(&output).into_rgba8();
            assert_eq!((decoded.width(), decoded.height()), expected_size);
            for (x, y, pixel) in source.enumerate_pixels() {
                let (dx, dy) = displayed_position(orientation, (3, 2), x, y);
                assert_eq!(
                    decoded.get_pixel(dx, dy),
                    pixel,
                    "orientation {orientation} at ({x}, {y})"
                );
            }
            assert_eq!(inspect_image(&output).unwrap().orientation, 1);
        }
    }

    #[test]
    fn applies_every_exif_orientation_from_jpeg_exactly_once() {
        let dir = tempdir().unwrap();
        // 3×2 cells of 16×16 flat colour survive JPEG compression.
        let cell = 16;
        let mut source = RgbImage::new(3 * cell, 2 * cell);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            let (cx, cy) = (x / cell, y / cell);
            *pixel = Rgb([(cx * 100 + 20) as u8, (cy * 150 + 40) as u8, 128]);
        }

        for orientation in 1..=8u16 {
            let input = dir.path().join(format!("o{orientation}.jpg"));
            write_jpeg_image(&input, &source, 100, Some(orientation));
            assert_eq!(
                inspect_image(&input).unwrap().orientation,
                orientation as u8
            );

            let output = dir.path().join(format!("o{orientation}.png"));
            let outcome =
                convert(&input, &output, ConversionOptions::new(ImageFormat::Png)).unwrap();
            let expected_size = displayed_size(orientation, (3 * cell, 2 * cell));
            assert_eq!(
                (outcome.width, outcome.height),
                expected_size,
                "orientation {orientation}"
            );

            let decoded = decode(&output).into_rgb8();
            for cx in 0..3 {
                for cy in 0..2 {
                    let (x, y) = (cx * cell + cell / 2, cy * cell + cell / 2);
                    let (dx, dy) = displayed_position(orientation, (3 * cell, 2 * cell), x, y);
                    let expected = source.get_pixel(x, y);
                    let actual = decoded.get_pixel(dx, dy);
                    for channel in 0..3 {
                        let difference = i32::from(expected[channel]) - i32::from(actual[channel]);
                        assert!(
                            difference.abs() <= 8,
                            "orientation {orientation} cell ({cx}, {cy})"
                        );
                    }
                }
            }
            assert_eq!(inspect_image(&output).unwrap().orientation, 1);
        }
    }

    #[test]
    fn jpeg_output_drops_exif_and_png_output_has_no_metadata_chunks() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("rotated.jpg");
        write_jpeg(&input, 40, 20, Some(6));

        let png = dir.path().join("out.png");
        convert(&input, &png, ConversionOptions::new(ImageFormat::Png)).unwrap();
        let bytes = fs::read(&png).unwrap();
        for chunk in [b"eXIf", b"iCCP"] {
            assert!(
                !bytes.windows(4).any(|window| window == chunk),
                "PNG carries {chunk:?}"
            );
        }

        let webp = dir.path().join("out.webp");
        convert(&input, &webp, ConversionOptions::new(ImageFormat::WebP)).unwrap();
        let jpeg = dir.path().join("out.jpg");
        convert(&webp, &jpeg, ConversionOptions::new(ImageFormat::Jpeg)).unwrap();
        let bytes = fs::read(&jpeg).unwrap();
        assert!(
            !bytes.windows(2).any(|window| window == [0xFF, 0xE1]),
            "JPEG carries APP1"
        );
        let source = inspect_image(&jpeg).unwrap();
        assert_eq!(
            (source.width, source.height, source.orientation),
            (20, 40, 1)
        );
    }

    #[test]
    fn jpeg_quality_changes_output_size_and_is_validated() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("photo.png");
        write_png(&png, 120, 90, false);

        let sizes: Vec<u64> = [30u8, 95]
            .into_iter()
            .map(|quality| {
                let output = dir.path().join(format!("q{quality}.jpg"));
                let options = ConversionOptions {
                    jpeg_quality: quality,
                    ..ConversionOptions::new(ImageFormat::Jpeg)
                };
                convert(&png, &output, options).unwrap().bytes_written
            })
            .collect();
        assert!(sizes[0] < sizes[1]);

        for quality in [0u8, 101] {
            let options = ConversionOptions {
                jpeg_quality: quality,
                ..ConversionOptions::new(ImageFormat::Jpeg)
            };
            let result = convert(&png, &dir.path().join("bad.jpg"), options);
            assert!(matches!(
                result,
                Err(ImageConversionError::InvalidQuality { .. })
            ));
        }

        // Quality is irrelevant for lossless output and never rejected there.
        let options = ConversionOptions {
            jpeg_quality: 0,
            ..ConversionOptions::new(ImageFormat::WebP)
        };
        convert(&png, &dir.path().join("fine.webp"), options).unwrap();
    }

    #[test]
    fn detects_format_from_content_not_extension() {
        let dir = tempdir().unwrap();
        let mislabelled = dir.path().join("actually-jpeg.png");
        write_jpeg(&mislabelled, 10, 10, None);

        assert_eq!(
            inspect_image(&mislabelled).unwrap().format,
            ImageFormat::Jpeg
        );
        let result = convert(
            &mislabelled,
            &dir.path().join("out.jpg"),
            ConversionOptions::new(ImageFormat::Jpeg),
        );
        assert!(matches!(
            result,
            Err(ImageConversionError::SameFormat { .. })
        ));

        let output = dir.path().join("out.png");
        convert(
            &mislabelled,
            &output,
            ConversionOptions::new(ImageFormat::Png),
        )
        .unwrap();
        assert_png(&output);
    }

    #[test]
    fn rejects_unsupported_malformed_and_animated_inputs() {
        let dir = tempdir().unwrap();

        let text = dir.path().join("notes.png");
        fs::write(&text, b"hello, this is not an image at all").unwrap();
        assert!(matches!(
            inspect_image(&text),
            Err(ImageConversionError::UnsupportedFormat { detected: None, .. })
        ));

        let gif = dir.path().join("anim.gif");
        fs::write(&gif, b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").unwrap();
        assert!(matches!(
            inspect_image(&gif),
            Err(ImageConversionError::UnsupportedFormat {
                detected: Some(_),
                ..
            })
        ));

        let truncated = dir.path().join("truncated.png");
        let good = dir.path().join("good.png");
        write_png(&good, 64, 64, false);
        let bytes = fs::read(&good).unwrap();
        fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        let output = dir.path().join("truncated.jpg");
        assert!(matches!(
            convert(
                &truncated,
                &output,
                ConversionOptions::new(ImageFormat::Jpeg)
            ),
            Err(ImageConversionError::InvalidImage { .. })
        ));
        assert!(!output.exists());

        let animated = dir.path().join("anim.webp");
        let mut bytes = b"RIFF\x1e\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&[0x02, 0, 0, 0, 7, 0, 0, 7, 0, 0]);
        bytes.extend_from_slice(b"ANIM\x06\x00\x00\x00\0\0\0\0\0\0");
        fs::write(&animated, bytes).unwrap();
        assert!(matches!(
            inspect_image(&animated),
            Err(ImageConversionError::AnimatedImage { .. })
        ));

        assert!(matches!(
            inspect_image(&dir.path().join("missing.png")),
            Err(ImageConversionError::InputNotFound { .. })
        ));
    }

    #[test]
    fn rejects_oversized_images_before_decoding() {
        let dir = tempdir().unwrap();

        // A PNG header claiming 20,000 × 20,000 is rejected on dimensions
        // without allocating for the pixels.
        let wide = dir.path().join("wide.png");
        fs::write(&wide, png_stub(20_000, 20_000, 8, 2)).unwrap();
        let result = inspect_image(&wide);
        assert!(
            matches!(
                result,
                Err(ImageConversionError::DimensionTooLarge { .. }
                    | ImageConversionError::DecodeLimit { .. })
            ),
            "{result:?}"
        );

        // 8,000 × 8,000 fits each side but exceeds 50 MP.
        let many = dir.path().join("many.png");
        fs::write(&many, png_stub(8_000, 8_000, 8, 2)).unwrap();
        assert!(matches!(
            inspect_image(&many),
            Err(ImageConversionError::TooManyPixels {
                width: 8_000,
                height: 8_000,
                ..
            })
        ));

        let huge = dir.path().join("huge.png");
        let file = fs::File::create(&huge).unwrap();
        file.set_len(MAX_INPUT_BYTES + 1).unwrap();
        assert!(matches!(
            inspect_image(&huge),
            Err(ImageConversionError::InputTooLarge { .. })
        ));
    }

    #[test]
    fn decoder_allocation_is_bounded() {
        // A 16-bit RGBA image at the pixel limit needs 400 MiB, above the cap.
        let dir = tempdir().unwrap();
        let deep = dir.path().join("deep.png");
        fs::write(&deep, png_stub(7_000, 7_000, 16, 6)).unwrap();

        assert!(matches!(
            inspect_image(&deep),
            Err(ImageConversionError::DecodeLimit { .. })
        ));
        let output = dir.path().join("deep.jpg");
        assert!(matches!(
            convert(&deep, &output, ConversionOptions::new(ImageFormat::Jpeg)),
            Err(ImageConversionError::DecodeLimit { .. })
        ));
        assert!(!output.exists());

        // The same dimensions at 8 bits (196 MB) are within the cap.
        let fine = dir.path().join("fine.png");
        fs::write(&fine, png_stub(7_000, 7_000, 8, 6)).unwrap();
        assert_eq!(inspect_image(&fine).unwrap().width, 7_000);
    }

    /// A structurally complete PNG whose header claims `width` × `height`.
    fn png_stub(width: u32, height: u32, bit_depth: u8, color_type: u8) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        for (chunk_type, data) in [
            (
                &b"IHDR"[..],
                [
                    &width.to_be_bytes()[..],
                    &height.to_be_bytes()[..],
                    &[bit_depth, color_type, 0, 0, 0][..],
                ]
                .concat(),
            ),
            (
                &b"IDAT"[..],
                vec![0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
            ),
            (&b"IEND"[..], Vec::new()),
        ] {
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend_from_slice(chunk_type);
            bytes.extend_from_slice(&data);
            bytes.extend_from_slice(&crc32(chunk_type, &data).to_be_bytes());
        }
        bytes
    }

    fn crc32(chunk_type: &[u8], data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in chunk_type.iter().chain(data) {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn never_overwrites_without_permission_and_keeps_the_source() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("photo.png");
        write_png(&png, 12, 12, false);
        let original = fs::read(&png).unwrap();
        let output = dir.path().join("photo.jpg");
        fs::write(&output, b"existing").unwrap();

        let result = convert(&png, &output, ConversionOptions::new(ImageFormat::Jpeg));
        assert!(matches!(
            result,
            Err(ImageConversionError::OutputExists { .. })
        ));
        assert_eq!(fs::read(&output).unwrap(), b"existing");

        convert_image_with_progress(
            &png,
            &output,
            ConversionOptions::new(ImageFormat::Jpeg),
            true,
            &mut (),
        )
        .unwrap();
        assert_jpeg(&output);
        assert_eq!(
            fs::read(&png).unwrap(),
            original,
            "source bytes are untouched"
        );

        let result = convert_image_with_progress(
            &png,
            &png,
            ConversionOptions::new(ImageFormat::Jpeg),
            true,
            &mut (),
        );
        assert!(matches!(
            result,
            Err(ImageConversionError::OutputMatchesInput { .. })
        ));
        assert_eq!(fs::read(&png).unwrap(), original);

        let missing_dir = dir.path().join("missing").join("photo.jpg");
        assert!(matches!(
            convert(
                &png,
                &missing_dir,
                ConversionOptions::new(ImageFormat::Jpeg)
            ),
            Err(ImageConversionError::OutputDirectoryInvalid { .. })
        ));
    }

    #[test]
    fn cancellation_leaves_no_output_or_temporary_files() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("photo.png");
        write_png(&png, 12, 12, false);
        let output = dir.path().join("photo.webp");

        // Steps: reading-image, decoding, orienting, encoding, saving-image.
        for cancel_at in 0..5 {
            let result = convert_image_with_progress(
                &png,
                &output,
                ConversionOptions::new(ImageFormat::WebP),
                false,
                &mut Recorder::cancelling_at(cancel_at),
            );
            assert!(
                matches!(result, Err(ImageConversionError::Cancelled)),
                "step {cancel_at}"
            );
            let entries: Vec<_> = fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(
                entries,
                vec![std::ffi::OsString::from("photo.png")],
                "step {cancel_at}"
            );
        }

        let mut recorder = Recorder::default();
        convert_image_with_progress(
            &png,
            &output,
            ConversionOptions::new(ImageFormat::WebP),
            false,
            &mut recorder,
        )
        .unwrap();
        let stages: Vec<_> = recorder.steps.iter().map(|(stage, _, _)| *stage).collect();
        assert_eq!(
            stages,
            [
                "reading-image",
                "decoding",
                "orienting",
                "encoding",
                "saving-image"
            ]
        );
    }

    #[test]
    fn inspect_reports_source_details_and_destinations() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("alpha.png");
        write_png(&png, 30, 20, true);

        let source = inspect_image(&png).unwrap();
        assert_eq!(
            source,
            ImageSource {
                format: ImageFormat::Png,
                width: 30,
                height: 20,
                bytes: fs::metadata(&png).unwrap().len(),
                has_alpha: true,
                orientation: 1,
            }
        );
        assert_eq!(
            source.format.destinations(),
            vec![ImageFormat::Jpeg, ImageFormat::WebP]
        );
        assert_eq!(
            ImageFormat::Jpeg.destinations(),
            vec![ImageFormat::Png, ImageFormat::WebP]
        );
    }

    /// Peak-memory probes for the largest allowed image. Run one at a time
    /// under `/usr/bin/time -l` (macOS) or `/usr/bin/time -v` (Linux):
    /// `cargo test --release --no-run`, then run the test binary with
    /// `--ignored --exact <name>`.
    #[test]
    #[ignore]
    fn probe_peak_memory_flatten_to_jpeg() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("big.png");
        let mut image = RgbaImage::new(7_000, 7_000);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([(x % 256) as u8, (y % 256) as u8, 90, (x % 2 * 255) as u8]);
        }
        image.save(&input).unwrap();
        drop(image);

        let outcome = convert(
            &input,
            &dir.path().join("big.jpg"),
            ConversionOptions::new(ImageFormat::Jpeg),
        )
        .unwrap();
        assert!(outcome.flattened);
    }

    #[test]
    #[ignore]
    fn probe_peak_memory_rotate_jpeg_to_png() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("big.jpg");
        let mut image = RgbImage::new(7_000, 7_000);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgb([(x % 256) as u8, (y % 256) as u8, 90]);
        }
        write_jpeg_image(&input, &image, 90, Some(6));
        drop(image);

        let outcome = convert(
            &input,
            &dir.path().join("big.png"),
            ConversionOptions::new(ImageFormat::Png),
        )
        .unwrap();
        assert_eq!((outcome.width, outcome.height), (7_000, 7_000));
    }

    #[test]
    fn parses_background_colours() {
        assert_eq!(parse_background("#ffffff"), Some([255, 255, 255]));
        assert_eq!(parse_background("#102030"), Some([16, 32, 48]));
        assert_eq!(parse_background("ffffff"), None);
        assert_eq!(parse_background("#fff"), None);
        assert_eq!(parse_background("#gg0000"), None);
    }
}
