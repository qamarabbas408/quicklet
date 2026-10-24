use std::fmt;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};

use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};
use tempfile::NamedTempFile;

pub const MAX_IMAGES: usize = 8;
pub const A4_WIDTH_PT: f32 = 595.0;
pub const A4_HEIGHT_PT: f32 = 842.0;
pub const MARGIN_PT: f32 = 56.7;

#[derive(Debug)]
pub enum PdfError {
    NoInputs,
    TooManyInputs { max: usize },
    InputNotFound { path: PathBuf },
    InputRead { path: PathBuf, source: io::Error },
    UnsupportedFormat { path: PathBuf, format: String },
    AnimatedWebP { path: PathBuf },
    InvalidImage { path: PathBuf, reason: String },
    UnsupportedColor { path: PathBuf, reason: String },
    OutputExists { path: PathBuf },
    OutputDirectoryInvalid { path: PathBuf },
    OutputWrite { path: PathBuf, source: io::Error },
}

impl fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoInputs => write!(formatter, "At least one image is required."),
            Self::TooManyInputs { max } => {
                write!(formatter, "At most {max} images can be combined.")
            }
            Self::InputNotFound { path } => {
                write!(formatter, "Image not found: {}", path.display())
            }
            Self::InputRead { path, source } => {
                write!(formatter, "Could not read {}: {source}", path.display())
            }
            Self::UnsupportedFormat { path, format } => {
                write!(
                    formatter,
                    "{} is not supported (detected {format}); use PNG, JPEG, or WebP.",
                    path.display()
                )
            }
            Self::AnimatedWebP { path } => {
                write!(
                    formatter,
                    "Animated WebP is not supported: {}",
                    path.display()
                )
            }
            Self::InvalidImage { path, reason } => {
                write!(formatter, "Could not decode {}: {reason}", path.display())
            }
            Self::UnsupportedColor { path, reason } => {
                write!(
                    formatter,
                    "Unsupported JPEG color in {}: {reason}",
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

impl std::error::Error for PdfError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InputRead { source, .. } | Self::OutputWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfOutcome {
    pub pages: usize,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
    draw_width: f32,
    draw_height: f32,
}

#[derive(Debug, Clone, Copy)]
enum ImageColorSpace {
    Rgb,
    Gray,
}

struct PageImage {
    width: u32,
    height: u32,
    orientation: u32,
    color_space: ImageColorSpace,
    filter: Filter,
    data: Vec<u8>,
    alpha: Option<Vec<u8>>,
}

/// Combines images into a single A4 PDF, one image per page.
///
/// Input paths are read in order. Existing output files are never overwritten.
pub fn convert_images_to_pdf(inputs: &[PathBuf], output: &Path) -> Result<PdfOutcome, PdfError> {
    convert(inputs, output, false)
}

/// Like [`convert_images_to_pdf`], but replaces an existing output file.
///
/// Use this only after the user has explicitly confirmed replacement in a
/// native save dialog.
pub fn convert_images_to_pdf_with_overwrite(
    inputs: &[PathBuf],
    output: &Path,
) -> Result<PdfOutcome, PdfError> {
    convert(inputs, output, true)
}

fn convert(
    inputs: &[PathBuf],
    output: &Path,
    allow_overwrite: bool,
) -> Result<PdfOutcome, PdfError> {
    if inputs.is_empty() {
        return Err(PdfError::NoInputs);
    }
    if inputs.len() > MAX_IMAGES {
        return Err(PdfError::TooManyInputs { max: MAX_IMAGES });
    }

    validate_output(output, allow_overwrite)?;

    let pages = inputs
        .iter()
        .map(|path| read_page(path))
        .collect::<Result<Vec<_>, _>>()?;
    let bytes = build_pdf(&pages);
    write_safely(output, &bytes, allow_overwrite)?;

    Ok(PdfOutcome {
        pages: pages.len(),
        bytes_written: bytes.len() as u64,
    })
}

fn validate_output(output: &Path, allow_overwrite: bool) -> Result<(), PdfError> {
    if output.exists() {
        if output.is_dir() {
            return Err(PdfError::OutputWrite {
                path: output.to_path_buf(),
                source: io::Error::new(io::ErrorKind::AlreadyExists, "output path is a directory"),
            });
        }
        if !allow_overwrite {
            return Err(PdfError::OutputExists {
                path: output.to_path_buf(),
            });
        }
    }

    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    if !parent.is_dir() {
        return Err(PdfError::OutputDirectoryInvalid {
            path: parent.to_path_buf(),
        });
    }

    Ok(())
}

fn read_page(path: &Path) -> Result<PageImage, PdfError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(PdfError::InputNotFound {
                path: path.to_path_buf(),
            })
        }
        Err(source) => {
            return Err(PdfError::InputRead {
                path: path.to_path_buf(),
                source,
            })
        }
    };

    let format = image::guess_format(&bytes).map_err(|error| PdfError::InvalidImage {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;

    match format {
        image::ImageFormat::Jpeg => read_jpeg(path, bytes),
        image::ImageFormat::Png => read_raster(path, bytes),
        image::ImageFormat::WebP => {
            if is_animated_webp(&bytes) {
                return Err(PdfError::AnimatedWebP {
                    path: path.to_path_buf(),
                });
            }
            read_raster(path, bytes)
        }
        other => Err(PdfError::UnsupportedFormat {
            path: path.to_path_buf(),
            format: format!("{other:?}"),
        }),
    }
}

fn read_jpeg(path: &Path, bytes: Vec<u8>) -> Result<PageImage, PdfError> {
    let info = jpeg_info(&bytes).ok_or_else(|| PdfError::InvalidImage {
        path: path.to_path_buf(),
        reason: "invalid JPEG structure".to_string(),
    })?;

    let color_space = match info.components {
        1 => ImageColorSpace::Gray,
        3 => ImageColorSpace::Rgb,
        components => {
            return Err(PdfError::UnsupportedColor {
                path: path.to_path_buf(),
                reason: format!("{components}-component JPEG"),
            })
        }
    };

    Ok(PageImage {
        width: info.width,
        height: info.height,
        orientation: jpeg_orientation(&bytes),
        color_space,
        filter: Filter::DctDecode,
        data: bytes,
        alpha: None,
    })
}

fn read_raster(path: &Path, bytes: Vec<u8>) -> Result<PageImage, PdfError> {
    let dynamic = image::load_from_memory(&bytes).map_err(|error| PdfError::InvalidImage {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let width = dynamic.width();
    let height = dynamic.height();

    if width == 0 || height == 0 {
        return Err(PdfError::InvalidImage {
            path: path.to_path_buf(),
            reason: "image has zero width or height".to_string(),
        });
    }

    let rgb = dynamic.to_rgb8();
    let data = compress(rgb.as_raw());

    let alpha = dynamic.color().has_alpha().then(|| {
        let rgba = dynamic.to_rgba8();
        let alphas: Vec<u8> = rgba.pixels().map(|pixel| pixel[3]).collect();
        compress(&alphas)
    });

    Ok(PageImage {
        width,
        height,
        orientation: 1,
        color_space: ImageColorSpace::Rgb,
        filter: Filter::FlateDecode,
        data,
        alpha,
    })
}

fn compress(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

fn is_animated_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 21
        && &bytes[0..4] == b"RIFF"
        && &bytes[8..12] == b"WEBP"
        && &bytes[12..16] == b"VP8X"
        && bytes[20] & 0x02 != 0
}

fn jpeg_orientation(bytes: &[u8]) -> u32 {
    let mut cursor = Cursor::new(bytes);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut cursor) else {
        return 1;
    };

    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .filter(|orientation| (1..=8).contains(orientation))
        .unwrap_or(1)
}

struct JpegInfo {
    width: u32,
    height: u32,
    components: u8,
}

fn jpeg_info(bytes: &[u8]) -> Option<JpegInfo> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }

    let mut index = 2;
    while index + 1 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        index += 2;

        if marker == 0xD8 || marker == 0xD9 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        if index + 2 > bytes.len() {
            return None;
        }
        let length = u16::from_be_bytes([bytes[index], bytes[index + 1]]) as usize;
        if length < 2 || index + length > bytes.len() {
            return None;
        }

        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof {
            let segment = &bytes[index + 2..index + length];
            if segment.len() < 6 {
                return None;
            }
            let height = u16::from_be_bytes([segment[1], segment[2]]) as u32;
            let width = u16::from_be_bytes([segment[3], segment[4]]) as u32;
            let components = segment[5];
            return Some(JpegInfo {
                width,
                height,
                components,
            });
        }

        index += length;
    }

    None
}

fn placement(width: u32, height: u32, orientation: u32) -> Placement {
    let orientation = if (1..=8).contains(&orientation) {
        orientation
    } else {
        1
    };
    let swapped = (5..=8).contains(&orientation);
    let display_width = if swapped { height as f32 } else { width as f32 };
    let display_height = if swapped { width as f32 } else { height as f32 };
    let available_width = A4_WIDTH_PT - 2.0 * MARGIN_PT;
    let available_height = A4_HEIGHT_PT - 2.0 * MARGIN_PT;
    let scale = (available_width / display_width)
        .min(available_height / display_height)
        .min(1.0);
    let draw_width = display_width * scale;
    let draw_height = display_height * scale;
    let x = (A4_WIDTH_PT - draw_width) / 2.0;
    let y = (A4_HEIGHT_PT - draw_height) / 2.0;

    let (a, b, c, d, e, f) = match orientation {
        2 => (-draw_width, 0.0, 0.0, draw_height, x + draw_width, y),
        3 => (
            -draw_width,
            0.0,
            0.0,
            -draw_height,
            x + draw_width,
            y + draw_height,
        ),
        4 => (draw_width, 0.0, 0.0, -draw_height, x, y + draw_height),
        5 => (
            0.0,
            -draw_height,
            -draw_width,
            0.0,
            x + draw_width,
            y + draw_height,
        ),
        6 => (0.0, -draw_height, draw_width, 0.0, x, y + draw_height),
        7 => (0.0, draw_height, draw_width, 0.0, x, y),
        8 => (0.0, draw_height, -draw_width, 0.0, x + draw_width, y),
        _ => (draw_width, 0.0, 0.0, draw_height, x, y),
    };

    Placement {
        a,
        b,
        c,
        d,
        e,
        f,
        draw_width,
        draw_height,
    }
}

fn build_pdf(pages: &[PageImage]) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let catalog = Ref::new(1);
    let page_tree = Ref::new(2);

    pdf.catalog(catalog).pages(page_tree);

    let page_ids: Vec<Ref> = (0..pages.len())
        .map(|index| Ref::new(3 + (index as i32) * 4))
        .collect();
    pdf.pages(page_tree)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);

    for (index, page_image) in pages.iter().enumerate() {
        let base = 3 + (index as i32) * 4;
        let page_id = Ref::new(base);
        let content_id = Ref::new(base + 1);
        let image_id = Ref::new(base + 2);
        let smask_id = Ref::new(base + 3);
        let name_string = format!("Im{index}");
        let name = Name(name_string.as_bytes());
        let placement = placement(page_image.width, page_image.height, page_image.orientation);

        {
            let mut page = pdf.page(page_id);
            page.media_box(Rect::new(0.0, 0.0, A4_WIDTH_PT, A4_HEIGHT_PT));
            page.parent(page_tree);
            page.contents(content_id);
            page.resources().x_objects().pair(name, image_id);
            page.finish();
        }

        {
            let mut xobject = pdf.image_xobject(image_id, &page_image.data);
            xobject.filter(page_image.filter);
            xobject.width(page_image.width as i32);
            xobject.height(page_image.height as i32);
            match page_image.color_space {
                ImageColorSpace::Rgb => xobject.color_space().device_rgb(),
                ImageColorSpace::Gray => xobject.color_space().device_gray(),
            };
            xobject.bits_per_component(8);
            if page_image.alpha.is_some() {
                xobject.s_mask(smask_id);
            }
            xobject.finish();
        }

        if let Some(alpha) = &page_image.alpha {
            let mut smask = pdf.image_xobject(smask_id, alpha);
            smask.filter(Filter::FlateDecode);
            smask.width(page_image.width as i32);
            smask.height(page_image.height as i32);
            smask.color_space().device_gray();
            smask.bits_per_component(8);
            smask.finish();
        }

        let mut content = Content::new();
        content.save_state();
        content.transform([
            placement.a,
            placement.b,
            placement.c,
            placement.d,
            placement.e,
            placement.f,
        ]);
        content.x_object(name);
        content.restore_state();
        pdf.stream(content_id, &content.finish());
    }

    pdf.finish()
}

fn write_safely(output: &Path, bytes: &[u8], allow_overwrite: bool) -> Result<(), PdfError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temp = NamedTempFile::new_in(parent).map_err(|source| PdfError::OutputWrite {
        path: output.to_path_buf(),
        source,
    })?;

    temp.write_all(bytes)
        .map_err(|source| PdfError::OutputWrite {
            path: output.to_path_buf(),
            source,
        })?;
    temp.as_file()
        .sync_all()
        .map_err(|source| PdfError::OutputWrite {
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
                Err(PdfError::OutputExists {
                    path: output.to_path_buf(),
                })
            } else {
                Err(PdfError::OutputWrite {
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
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::webp::WebPEncoder;
    use image::{ExtendedColorType, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
    use lopdf::{Document, ObjectId};
    use std::fs;
    use tempfile::tempdir;

    fn write_png(path: &Path, width: u32, height: u32, alpha: bool) {
        let mut image = RgbaImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let band = x * 4 / width.max(1);
            let color: [u8; 3] = match band {
                0 => [230, 60, 70],
                1 => [60, 180, 90],
                2 => [60, 90, 230],
                _ => [240, 200, 60],
            };
            let alpha = if alpha && x < width / 2 { 0 } else { 255 };
            let shade = (y * 30 / height.max(1)) as u8;
            *pixel = Rgba([
                color[0].saturating_add(shade),
                color[1].saturating_add(shade),
                color[2].saturating_add(shade),
                alpha,
            ]);
        }
        image.save_with_format(path, ImageFormat::Png).unwrap();
    }

    fn write_jpeg(path: &Path, width: u32, height: u32, orientation: Option<u16>) {
        let mut image = RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgb([
                (x * 255 / width.max(1)) as u8,
                (y * 255 / height.max(1)) as u8,
                128,
            ]);
        }
        let mut file = fs::File::create(path).unwrap();
        JpegEncoder::new_with_quality(&mut file, 90)
            .encode(image.as_raw(), width, height, ExtendedColorType::Rgb8)
            .unwrap();
        drop(file);

        if let Some(orientation) = orientation {
            let bytes = fs::read(path).unwrap();
            let segment = exif_orientation_segment(orientation);
            let mut with_exif = Vec::with_capacity(bytes.len() + segment.len());
            with_exif.extend_from_slice(&bytes[..2]);
            with_exif.extend_from_slice(&segment);
            with_exif.extend_from_slice(&bytes[2..]);
            fs::write(path, with_exif).unwrap();
        }
    }

    fn exif_orientation_segment(orientation: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&0u16.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let length = (payload.len() + 2) as u16;

        let mut segment = vec![0xFF, 0xE1];
        segment.extend_from_slice(&length.to_be_bytes());
        segment.extend_from_slice(&payload);
        segment
    }

    fn write_webp(path: &Path, width: u32, height: u32) {
        let mut image = RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let stripe = (x / 5) % 2 == 0;
            let shade = (y * 40 / height.max(1)) as u8;
            *pixel = if stripe {
                Rgb([40 + shade, 130, 220])
            } else {
                Rgb([245, 245, 245])
            };
        }
        let file = fs::File::create(path).unwrap();
        WebPEncoder::new_lossless(file)
            .encode(image.as_raw(), width, height, ExtendedColorType::Rgb8)
            .unwrap();
    }

    fn write_large_png(path: &Path) {
        let width = 3000;
        let height = 2000;
        let mut image = RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
        }
        image.save_with_format(path, ImageFormat::Png).unwrap();
    }

    fn page_ids(doc: &Document) -> Vec<ObjectId> {
        doc.get_pages().into_values().collect()
    }

    fn page_image_stream(doc: &Document, page_id: ObjectId) -> &lopdf::Stream {
        let page = doc.get_object(page_id).unwrap().as_dict().unwrap();
        let resources = page.get(b"Resources").unwrap().as_dict().unwrap();
        let xobjects = resources.get(b"XObject").unwrap().as_dict().unwrap();
        let (_, reference) = xobjects.iter().next().unwrap();
        let object_id = reference.as_reference().unwrap();
        doc.get_object(object_id).unwrap().as_stream().unwrap()
    }

    fn image_widths(doc: &Document) -> Vec<i64> {
        page_ids(doc)
            .into_iter()
            .map(|page_id| {
                page_image_stream(doc, page_id)
                    .dict
                    .get(b"Width")
                    .unwrap()
                    .as_i64()
                    .unwrap()
            })
            .collect()
    }

    fn content_matrix(doc: &Document, page_id: ObjectId) -> Vec<f32> {
        let content = String::from_utf8_lossy(&doc.get_page_content(page_id)).into_owned();
        let line = content
            .lines()
            .find(|line| line.trim_end().ends_with(" cm"))
            .expect("content should contain a cm transform");
        line.split_whitespace()
            .take(6)
            .map(|value| value.parse::<f32>().unwrap())
            .collect()
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn creates_pages_in_input_order_and_keeps_aspect_ratio() {
        let dir = tempdir().unwrap();
        let png = dir.path().join("first.png");
        let jpeg = dir.path().join("second.jpg");
        let webp = dir.path().join("third.webp");
        let output = dir.path().join("ordered.pdf");

        write_png(&png, 120, 80, false);
        write_jpeg(&jpeg, 180, 90, None);
        write_webp(&webp, 240, 40);

        let outcome = convert_images_to_pdf(&[png, jpeg, webp], &output).unwrap();
        assert_eq!(outcome.pages, 3);

        let bytes = fs::read(&output).unwrap();
        let doc = Document::load_mem(&bytes).unwrap();
        assert_eq!(doc.get_pages().len(), 3);
        assert_eq!(image_widths(&doc), vec![120, 180, 240]);

        let first_page = page_ids(&doc)[0];
        let page = doc.get_object(first_page).unwrap().as_dict().unwrap();
        let media_box = page.get(b"MediaBox").unwrap().as_array().unwrap();
        let values: Vec<f32> = media_box
            .iter()
            .map(|value| value.as_float().unwrap())
            .collect();
        assert_close(values[2], A4_WIDTH_PT);
        assert_close(values[3], A4_HEIGHT_PT);

        let first_placement = placement(120, 80, 1);
        assert_close(
            first_placement.draw_width / first_placement.draw_height,
            1.5,
        );
    }

    #[test]
    fn embeds_png_alpha_as_smask() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("alpha.png");
        let output = dir.path().join("alpha.pdf");
        write_png(&input, 40, 20, true);

        convert_images_to_pdf(&[input], &output).unwrap();
        let bytes = fs::read(&output).unwrap();
        let doc = Document::load_mem(&bytes).unwrap();
        let page_id = page_ids(&doc)[0];
        let stream = page_image_stream(&doc, page_id);

        let smask_ref = stream
            .dict
            .get(b"SMask")
            .expect("SMask should exist")
            .as_reference()
            .unwrap();
        let smask = doc.get_object(smask_ref).unwrap().as_stream().unwrap();
        let alpha = smask.decompressed_content().unwrap();

        assert_eq!(alpha.len(), 40 * 20);
        assert_eq!(*alpha.iter().min().unwrap(), 0);
        assert_eq!(*alpha.iter().max().unwrap(), 255);
    }

    #[test]
    fn embeds_jpeg_without_recompression_and_applies_exif_orientation() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("rotated.jpg");
        let output = dir.path().join("rotated.pdf");
        write_jpeg(&input, 40, 20, Some(6));
        let source = fs::read(&input).unwrap();
        assert_eq!(jpeg_orientation(&source), 6);

        convert_images_to_pdf(&[input], &output).unwrap();
        let bytes = fs::read(&output).unwrap();
        let doc = Document::load_mem(&bytes).unwrap();
        let page_id = page_ids(&doc)[0];
        let stream = page_image_stream(&doc, page_id);

        assert_eq!(
            stream.dict.get(b"Filter").unwrap().as_name().unwrap(),
            b"DCTDecode".as_slice()
        );
        assert_eq!(stream.content, source);

        let matrix = content_matrix(&doc, page_id);
        let expected = placement(40, 20, 6);
        for (actual, expected) in matrix.iter().zip([
            expected.a, expected.b, expected.c, expected.d, expected.e, expected.f,
        ]) {
            assert_close(*actual, expected);
        }
    }

    fn mapped(placement: Placement, u: f32, v: f32) -> (f32, f32) {
        (
            placement.a * u + placement.c * v + placement.e,
            placement.b * u + placement.d * v + placement.f,
        )
    }

    #[test]
    fn does_not_upscale_small_images() {
        let small = placement(100, 50, 1);
        assert_close(small.draw_width, 100.0);
        assert_close(small.draw_height, 50.0);

        let large = placement(4000, 3000, 1);
        assert!(large.draw_width <= A4_WIDTH_PT - 2.0 * MARGIN_PT + 0.01);
        assert!(large.draw_height <= A4_HEIGHT_PT - 2.0 * MARGIN_PT + 0.01);
    }

    #[test]
    fn placement_maps_exif_corners_correctly() {
        let cases = [
            (1, (0.0, 0.0), (0.0, 0.0)),
            (2, (0.0, 1.0), (1.0, 1.0)),
            (3, (0.0, 1.0), (1.0, 0.0)),
            (4, (0.0, 1.0), (0.0, 0.0)),
            (5, (1.0, 1.0), (0.0, 0.0)),
            (6, (0.0, 1.0), (1.0, 1.0)),
            (7, (0.0, 1.0), (1.0, 0.0)),
            (8, (0.0, 1.0), (0.0, 0.0)),
        ];

        for (orientation, (u, v), (expected_u, expected_v)) in cases {
            let placement = placement(200, 100, orientation);
            let x = (A4_WIDTH_PT - placement.draw_width) / 2.0;
            let y = (A4_HEIGHT_PT - placement.draw_height) / 2.0;
            let (actual_x, actual_y) = mapped(placement, u, v);
            assert_close(actual_x, x + expected_u * placement.draw_width);
            assert_close(actual_y, y + expected_v * placement.draw_height);
        }
    }

    #[test]
    fn placement_handles_all_exif_orientations() {
        let normal = placement(200, 100, 1);
        assert_close(normal.draw_width / normal.draw_height, 2.0);

        let rotated = placement(200, 100, 6);
        assert_close(rotated.draw_width / rotated.draw_height, 0.5);

        for orientation in 1..=8 {
            let placement = placement(200, 100, orientation);
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let x = placement.a * u + placement.c * v + placement.e;
                let y = placement.b * u + placement.d * v + placement.f;
                assert!((-0.01..=A4_WIDTH_PT + 0.01).contains(&x));
                assert!((-0.01..=A4_HEIGHT_PT + 0.01).contains(&y));
            }
        }
    }

    #[test]
    fn rejects_invalid_and_unsupported_inputs() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("invalid.pdf");

        let missing = dir.path().join("missing.png");
        assert!(matches!(
            convert_images_to_pdf(&[missing], &output),
            Err(PdfError::InputNotFound { .. })
        ));

        let text = dir.path().join("not-an-image.png");
        fs::write(&text, b"hello world").unwrap();
        assert!(matches!(
            convert_images_to_pdf(&[text], &output),
            Err(PdfError::InvalidImage { .. })
        ));

        let gif = dir.path().join("image.gif");
        fs::write(&gif, b"GIF89a").unwrap();
        assert!(matches!(
            convert_images_to_pdf(&[gif], &output),
            Err(PdfError::UnsupportedFormat { .. })
        ));

        let corrupt = dir.path().join("corrupt.png");
        let mut corrupt_bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        corrupt_bytes.extend_from_slice(b"broken");
        fs::write(&corrupt, corrupt_bytes).unwrap();
        assert!(matches!(
            convert_images_to_pdf(&[corrupt], &output),
            Err(PdfError::InvalidImage { .. })
        ));

        let corrupt_jpeg = dir.path().join("corrupt.jpg");
        fs::write(&corrupt_jpeg, [0xFF, 0xD8, 0xFF, 0x00, 0x01]).unwrap();
        assert!(matches!(
            convert_images_to_pdf(&[corrupt_jpeg], &output),
            Err(PdfError::InvalidImage { .. })
        ));

        assert!(!output.exists());
    }

    #[test]
    fn rejects_animated_webp() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("animated.webp");
        let output = dir.path().join("animated.pdf");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&20u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"VP8X");
        bytes.extend_from_slice(&10u32.to_le_bytes());
        bytes.push(0x02);
        bytes.extend_from_slice(&[0; 17]);
        fs::write(&input, bytes).unwrap();

        assert!(matches!(
            convert_images_to_pdf(&[input], &output),
            Err(PdfError::AnimatedWebP { .. })
        ));
    }

    #[test]
    fn decodes_static_lossy_webp() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lossy.webp");
        let bytes = fs::read(&fixture).unwrap();
        assert!(
            bytes.windows(4).any(|window| window == b"VP8 "),
            "fixture should contain a lossy VP8 bitstream"
        );
        assert!(!is_animated_webp(&bytes), "fixture should be static");

        let dir = tempdir().unwrap();
        let output = dir.path().join("lossy.pdf");
        let outcome = convert_images_to_pdf(&[fixture], &output).unwrap();
        assert_eq!(outcome.pages, 1);

        let pdf_bytes = fs::read(&output).unwrap();
        let doc = Document::load_mem(&pdf_bytes).unwrap();
        assert_eq!(image_widths(&doc), vec![64]);
    }

    #[test]
    fn handles_large_images() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("large.png");
        let output = dir.path().join("large.pdf");
        write_large_png(&input);

        let outcome = convert_images_to_pdf(&[input], &output).unwrap();
        assert_eq!(outcome.pages, 1);

        let bytes = fs::read(&output).unwrap();
        let doc = Document::load_mem(&bytes).unwrap();
        assert_eq!(image_widths(&doc), vec![3000]);
    }

    #[test]
    fn never_overwrites_and_does_not_leave_temp_files() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("image.png");
        let output = dir.path().join("existing.pdf");
        write_png(&input, 20, 10, false);

        fs::write(&output, b"sentinel").unwrap();
        assert!(matches!(
            convert_images_to_pdf(std::slice::from_ref(&input), &output),
            Err(PdfError::OutputExists { .. })
        ));
        assert_eq!(fs::read(&output).unwrap(), b"sentinel");

        let fresh_output = dir.path().join("fresh.pdf");
        convert_images_to_pdf(&[input], &fresh_output).unwrap();

        let leftover = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp"))
            .count();
        assert_eq!(leftover, 0);
    }

    #[test]
    fn overwrite_error_does_not_dump_the_full_path() {
        let error = PdfError::OutputExists {
            path: PathBuf::from("/very/long/path/WhatsApp Image 2026-09-10.pdf"),
        };
        let message = error.to_string();

        assert!(message.contains("WhatsApp Image 2026-09-10.pdf"));
        assert!(!message.contains("/very/long/path"));
    }

    #[test]
    fn overwrites_existing_output_only_when_explicitly_allowed() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("image.png");
        let output = dir.path().join("result.pdf");
        write_png(&input, 20, 10, false);
        fs::write(&output, b"old content").unwrap();

        assert!(matches!(
            convert_images_to_pdf(std::slice::from_ref(&input), &output),
            Err(PdfError::OutputExists { .. })
        ));
        assert_eq!(fs::read(&output).unwrap(), b"old content");

        convert_images_to_pdf_with_overwrite(&[input], &output).unwrap();
        let bytes = fs::read(&output).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn reports_missing_output_directory() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("image.png");
        write_png(&input, 20, 10, false);

        let output = dir.path().join("missing").join("out.pdf");
        assert!(matches!(
            convert_images_to_pdf(&[input], &output),
            Err(PdfError::OutputDirectoryInvalid { .. })
        ));
        assert!(!output.exists());
    }

    #[test]
    fn enforces_input_limit_and_empty_input() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            convert_images_to_pdf(&[], &dir.path().join("empty.pdf")),
            Err(PdfError::NoInputs)
        ));

        let mut inputs = Vec::new();
        for index in 0..=MAX_IMAGES {
            let path = dir.path().join(format!("image-{index}.png"));
            write_png(&path, 10, 10, false);
            inputs.push(path);
        }

        assert!(matches!(
            convert_images_to_pdf(&inputs, &dir.path().join("many.pdf")),
            Err(PdfError::TooManyInputs { max: MAX_IMAGES })
        ));
    }
}
