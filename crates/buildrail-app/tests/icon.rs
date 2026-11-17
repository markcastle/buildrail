//! The checked-in `assets/buildrail.ico` (embedded in the Windows exe by
//! `build.rs`) is generated from [`buildrail_app::ui::icon::rgba`], the
//! same function the window icon uses:
//!
//! ```text
//! BUILDRAIL_WRITE_ICON=1 cargo test -p buildrail-app --test icon -- --ignored
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;
use std::path::PathBuf;

use buildrail_app::ui::icon::rgba;
use image::{ImageFormat, RgbaImage};

/// Sizes Windows asks for (title bar, taskbar, Explorer views).
const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

fn ico_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/buildrail.ico")
}

/// An ICO file with one PNG-compressed image per size.
fn ico() -> Vec<u8> {
    let images: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&size| {
            let image = RgbaImage::from_raw(size, size, rgba(size)).unwrap();
            let mut png = Cursor::new(Vec::new());
            image.write_to(&mut png, ImageFormat::Png).unwrap();
            png.into_inner()
        })
        .collect();
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (size, png) in SIZES.iter().zip(&images) {
        let edge = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([edge, edge, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((png.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in images {
        out.extend(png);
    }
    out
}

#[test]
#[ignore = "writes assets/buildrail.ico"]
fn write_icon() {
    if std::env::var("BUILDRAIL_WRITE_ICON").as_deref() == Ok("1") {
        std::fs::write(ico_path(), ico()).unwrap();
    }
}

#[test]
fn checked_in_icon_has_every_size() {
    let bytes = std::fs::read(ico_path()).expect("assets/buildrail.ico exists");
    assert_eq!(&bytes[..4], &[0, 0, 1, 0], "an ICO header");
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    assert_eq!(count, SIZES.len());
    for (index, size) in SIZES.iter().enumerate() {
        let entry = &bytes[6 + 16 * index..6 + 16 * (index + 1)];
        let edge = if *size >= 256 { 0 } else { *size as u8 };
        assert_eq!(entry[0], edge);
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        // Each image is a PNG of the right size, and draws the current mark.
        let png = image::load_from_memory_with_format(&bytes[offset..], ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(png.dimensions(), (*size, *size));
        assert_eq!(png.into_raw(), rgba(*size), "regenerate the icon ({size} px)");
    }
}
