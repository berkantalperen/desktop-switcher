//! The app's icon, drawn in code.
//!
//! A monitor whose screen is split between two colours: one screen, two
//! sources. Drawn at whatever size is asked for, so it stays sharp at any
//! display scaling, and in colours that read on light and dark taskbars alike,
//! with no outline that would vanish on one of them.
//!
//! One drawing serves everything: the tray draws it live, the GUI hands it to
//! its window, and each program's build script packs it into an `.ico` that
//! goes inside the executable, which is where the taskbar and Start Menu look.
//! No image file is kept in the repository to drift from the code.

const BLUE: [u8; 3] = [0x3B, 0x82, 0xF6];
const AMBER: [u8; 3] = [0xF5, 0x9E, 0x0B];
const GREY: [u8; 3] = [0x9C, 0xA3, 0xAF];

/// Samples per pixel along each axis, for smooth edges.
const SUPERSAMPLE: u32 = 4;

/// RGBA, row-major, top row first, straight (not premultiplied) alpha.
pub fn pixels(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f32;

    for py in 0..size {
        for px in 0..size {
            let mut rgb = [0f32; 3];
            let mut covered = 0f32;
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let x = (px as f32 + (sx as f32 + 0.5) / SUPERSAMPLE as f32) / s;
                    let y = (py as f32 + (sy as f32 + 0.5) / SUPERSAMPLE as f32) / s;
                    if let Some(colour) = shade(x, y) {
                        for c in 0..3 {
                            rgb[c] += colour[c] as f32;
                        }
                        covered += 1.0;
                    }
                }
            }
            let i = ((py * size + px) * 4) as usize;
            if covered > 0.0 {
                for c in 0..3 {
                    out[i + c] = (rgb[c] / covered).round() as u8;
                }
                out[i + 3] = (covered / samples * 255.0).round() as u8;
            }
        }
    }
    out
}

/// The colour at a point in the unit square, if anything is drawn there.
fn shade(x: f32, y: f32) -> Option<[u8; 3]> {
    // Screen.
    if in_rounded_rect(x, y, 0.04, 0.10, 0.96, 0.70, 0.09) {
        return Some(if x < 0.5 { BLUE } else { AMBER });
    }
    // Neck.
    if (0.44..0.56).contains(&x) && (0.70..0.84).contains(&y) {
        return Some(GREY);
    }
    // Foot.
    if in_rounded_rect(x, y, 0.26, 0.82, 0.74, 0.93, 0.05) {
        return Some(GREY);
    }
    None
}

fn in_rounded_rect(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32, r: f32) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let cx = x.clamp(left + r, right - r);
    let cy = y.clamp(top + r, bottom - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// The sizes Windows asks an executable's icon for, at every scaling step.
pub const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// A Windows `.ico` file holding the icon at each of `sizes`.
///
/// Each image is stored as a 32-bit bitmap with alpha, the form every version
/// of Windows reads, followed by the all-zero AND mask the format still
/// requires and which alpha makes irrelevant.
pub fn ico_file(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| ico_image(s)).collect();

    let mut out = Vec::new();
    // ICONDIR: reserved, type 1 (icon), image count.
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());

    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&size, image) in sizes.iter().zip(&images) {
        // A dimension of 256 is written as 0; the field is one byte.
        let dim = if size >= 256 { 0 } else { size as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0); // palette size
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        out.extend_from_slice(&image);
    }
    out
}

fn ico_image(size: u32) -> Vec<u8> {
    let rgba = pixels(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();

    // BITMAPINFOHEADER. The height counts the colour image and the mask.
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(size * size * 4 + mask_row * size).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // resolution and palette fields

    // Colour, bottom row first, as BGRA.
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.extend(std::iter::repeat_n(0u8, (mask_row * size) as usize));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }

    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    #[test]
    fn an_ico_file_declares_each_image_where_it_is() {
        let ico = ico_file(&ICO_SIZES);
        assert_eq!(u16_at(&ico, 0), 0);
        assert_eq!(u16_at(&ico, 2), 1, "type must be icon");
        assert_eq!(u16_at(&ico, 4) as usize, ICO_SIZES.len());

        let mut expected_offset = 6 + 16 * ICO_SIZES.len() as u32;
        for (n, &size) in ICO_SIZES.iter().enumerate() {
            let entry = 6 + 16 * n;
            let dim = if size == 256 { 0 } else { size as u8 };
            assert_eq!(ico[entry], dim, "width of {size}");
            assert_eq!(ico[entry + 1], dim, "height of {size}");
            assert_eq!(u16_at(&ico, entry + 6), 32);
            let length = u32_at(&ico, entry + 8);
            let offset = u32_at(&ico, entry + 12);
            assert_eq!(offset, expected_offset, "offset of {size}");
            // The bitmap header at that offset agrees on the size.
            assert_eq!(u32_at(&ico, offset as usize), 40);
            assert_eq!(u32_at(&ico, offset as usize + 4), size);
            assert_eq!(u32_at(&ico, offset as usize + 8), size * 2);
            expected_offset += length;
        }
        assert_eq!(
            expected_offset as usize,
            ico.len(),
            "no bytes unaccounted for"
        );
    }

    /// Bitmaps in an .ico are stored bottom row first, in BGRA.
    #[test]
    fn ico_pixels_are_bottom_up_bgra() {
        let size = 32;
        let ico = ico_file(&[size]);
        let pixels_start = 6 + 16 + 40;
        let rgba = pixels(size);
        // The first stored pixel is the bottom-left one.
        let src = (((size - 1) * size) * 4) as usize;
        assert_eq!(
            &ico[pixels_start..pixels_start + 4],
            &[rgba[src + 2], rgba[src + 1], rgba[src], rgba[src + 3]]
        );
        // A pixel on the screen, located from the top in the drawing.
        let (x, y) = (8u32, 12u32);
        let stored = pixels_start + (((size - 1 - y) * size + x) * 4) as usize;
        assert_eq!(&ico[stored..stored + 3], &[BLUE[2], BLUE[1], BLUE[0]]);
    }

    fn at(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn every_size_a_taskbar_asks_for_is_drawn() {
        for size in [16, 20, 24, 32, 48] {
            assert_eq!(pixels(size).len(), (size * size * 4) as usize);
        }
    }

    #[test]
    fn the_corners_are_transparent() {
        let px = pixels(32);
        for (x, y) in [(0, 0), (31, 0), (0, 31), (31, 31)] {
            assert_eq!(at(&px, 32, x, y)[3], 0, "corner {x},{y}");
        }
    }

    #[test]
    fn the_screen_shows_two_sources() {
        let px = pixels(32);
        let left = at(&px, 32, 8, 12);
        let right = at(&px, 32, 23, 12);
        assert_eq!(&left[..3], &BLUE);
        assert_eq!(&right[..3], &AMBER);
        assert_eq!(left[3], 255);
    }

    #[test]
    fn the_stand_is_drawn() {
        let px = pixels(32);
        assert_eq!(&at(&px, 32, 16, 28)[..3], &GREY);
    }
}

/// For build scripts: give a Windows executable its icon and a name.
#[cfg(feature = "build")]
pub mod build {
    use std::path::PathBuf;

    /// Embed the icon, and `description` as the name Windows shows for the
    /// program — in the taskbar's list of tray icons, in Task Manager, in a
    /// file's properties. Does nothing when the target is not Windows.
    pub fn embed(description: &str) {
        println!("cargo:rerun-if-changed=build.rs");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
        let ico = out.join("app.ico");
        std::fs::write(&ico, super::ico_file(&super::ICO_SIZES)).expect("writing the icon");

        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        let numbers: Vec<u16> = version
            .split(['.', '-', '+'])
            .take(3)
            .map(|n| n.parse().unwrap_or(0))
            .chain(std::iter::repeat(0))
            .take(3)
            .collect();
        let (major, minor, patch) = (numbers[0], numbers[1], numbers[2]);
        // Resource scripts treat a backslash as an escape, so double them.
        let path = ico.display().to_string().replace('\\', "\\\\");
        let text = |s: &str| s.replace('"', "\"\"");

        let rc = format!(
            r#"1 ICON "{path}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "{description}"
      VALUE "ProductName", "Desktop Switcher"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            description = text(description),
            version = text(&version),
        );
        let rc_path = out.join("app.rc");
        std::fs::write(&rc_path, rc).expect("writing the resource script");
        embed_resource::compile(&rc_path, embed_resource::NONE)
            .manifest_optional()
            .expect("compiling the Windows resources");
    }
}
