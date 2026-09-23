//! The tray icon, drawn in code.
//!
//! A monitor whose screen is split between two colours: one screen, two
//! sources. Drawn at whatever size the taskbar asks for, so it stays sharp at
//! any display scaling, and in colours that read on light and dark taskbars
//! alike, with no outline that would vanish on one of them.

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

#[cfg(test)]
mod tests {
    use super::*;

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
