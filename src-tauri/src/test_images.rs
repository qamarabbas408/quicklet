//! Image fixtures shared by module tests.

use std::fs;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

pub fn write_png(path: &Path, width: u32, height: u32, alpha: bool) {
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

/// Writes `image` as PNG with an `eXIf` chunk when `orientation` is given.
pub fn write_png_image(path: &Path, image: &RgbaImage, orientation: Option<u16>) {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .unwrap();

    if let Some(orientation) = orientation {
        let exif = exif_tiff(orientation);
        let chunk = png_chunk(b"eXIf", &exif);
        // Signature + IHDR length/type/data/CRC.
        let ihdr_end = 8 + 4 + 4 + 13 + 4;
        bytes.splice(ihdr_end..ihdr_end, chunk);
    }
    fs::write(path, bytes).unwrap();
}

fn png_chunk(chunk_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::with_capacity(12 + data.len());
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(chunk_type);
    chunk.extend_from_slice(data);
    chunk.extend_from_slice(&crc32(chunk_type, data).to_be_bytes());
    chunk
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

pub fn write_jpeg(path: &Path, width: u32, height: u32, orientation: Option<u16>) {
    let mut image = RgbImage::new(width, height);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = Rgb([
            (x * 255 / width.max(1)) as u8,
            (y * 255 / height.max(1)) as u8,
            128,
        ]);
    }
    write_jpeg_image(path, &image, 90, orientation);
}

/// Encodes `image` at `quality`, inserting an EXIF APP1 segment with
/// `orientation` when given.
pub fn write_jpeg_image(path: &Path, image: &RgbImage, quality: u8, orientation: Option<u16>) {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();

    if let Some(orientation) = orientation {
        let segment = exif_orientation_segment(orientation);
        let mut with_exif = Vec::with_capacity(bytes.len() + segment.len());
        with_exif.extend_from_slice(&bytes[..2]);
        with_exif.extend_from_slice(&segment);
        with_exif.extend_from_slice(&bytes[2..]);
        bytes = with_exif;
    }
    fs::write(path, bytes).unwrap();
}

/// A minimal little-endian TIFF holding only an Orientation tag.
pub fn exif_tiff(orientation: u16) -> Vec<u8> {
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
    tiff
}

/// A JPEG APP1 segment carrying [`exif_tiff`].
pub fn exif_orientation_segment(orientation: u16) -> Vec<u8> {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&exif_tiff(orientation));
    let length = (payload.len() + 2) as u16;

    let mut segment = vec![0xFF, 0xE1];
    segment.extend_from_slice(&length.to_be_bytes());
    segment.extend_from_slice(&payload);
    segment
}

pub fn write_webp(path: &Path, width: u32, height: u32) {
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

/// Writes `image` as lossless WebP, wrapped in the extended (VP8X) container
/// with an EXIF chunk holding `orientation` when given.
pub fn write_webp_image(path: &Path, image: &RgbaImage, orientation: Option<u16>) {
    let mut simple = Vec::new();
    WebPEncoder::new_lossless(&mut simple)
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .unwrap();

    let Some(orientation) = orientation else {
        fs::write(path, simple).unwrap();
        return;
    };

    // The simple container is RIFF <size> WEBP followed by one VP8L chunk.
    let vp8l = &simple[12..];

    let mut vp8x = Vec::new();
    vp8x.extend_from_slice(b"VP8X");
    vp8x.extend_from_slice(&10u32.to_le_bytes());
    vp8x.push(0x08 | 0x10); // EXIF and alpha flags.
    vp8x.extend_from_slice(&[0, 0, 0]);
    vp8x.extend_from_slice(&(image.width() - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(image.height() - 1).to_le_bytes()[..3]);

    let tiff = exif_tiff(orientation);
    let mut exif = Vec::new();
    exif.extend_from_slice(b"EXIF");
    exif.extend_from_slice(&(tiff.len() as u32).to_le_bytes());
    exif.extend_from_slice(&tiff);
    if tiff.len() % 2 == 1 {
        exif.push(0);
    }

    let body_len = 4 + vp8x.len() + vp8l.len() + exif.len();
    let mut bytes = Vec::with_capacity(body_len + 8);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(body_len as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&vp8x);
    bytes.extend_from_slice(vp8l);
    bytes.extend_from_slice(&exif);
    fs::write(path, bytes).unwrap();
}
