//! The tray icon: a white heart on a rounded rose square, matching the heart in Sortier's title bar. Drawn here
//! rather than loaded, so there's no file to ship.

use tray_icon::Icon;

/// Sortier's accent, `ROSE_500` in sortier-ui's tokens (which this crate doesn't depend on).
const ROSE: [f32; 3] = [237.0, 37.0, 83.0];
const SIZE: u32 = 32;
/// Samples per pixel along each side, for smooth edges.
const SAMPLES: u32 = 4;
/// The square's corner radius, as a share of its side.
const CORNER: f32 = 0.25;
/// How many heart-curve units span the icon's side; smaller makes the heart bigger.
const HEART_SCALE: f32 = 3.6;

pub fn heart() -> Icon {
    Icon::from_rgba(heart_pixels(), SIZE, SIZE).expect("the icon's pixels match its size")
}

/// The icon as straight RGBA8, `SIZE` pixels square.
fn heart_pixels() -> Vec<u8> {
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let samples = (SAMPLES * SAMPLES) as f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (mut square, mut heart) = (0.0_f32, 0.0_f32);
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let u = (x as f32 + (sample_x as f32 + 0.5) / SAMPLES as f32) / SIZE as f32;
                    let v = (y as f32 + (sample_y as f32 + 0.5) / SAMPLES as f32) / SIZE as f32;
                    if in_rounded_square(u, v) {
                        square += 1.0;
                        if in_heart(u, v) {
                            heart += 1.0;
                        }
                    }
                }
            }
            let white = if square > 0.0 { heart / square } else { 0.0 };
            for channel in ROSE {
                rgba.push((channel + (255.0 - channel) * white).round() as u8);
            }
            rgba.push((square / samples * 255.0).round() as u8);
        }
    }
    rgba
}

/// Whether (`u`, `v`), each 0 to 1 across the icon, is inside the rounded square filling it.
fn in_rounded_square(u: f32, v: f32) -> bool {
    let reach = 0.5 - CORNER;
    let (dx, dy) = (((u - 0.5).abs() - reach).max(0.0), ((v - 0.5).abs() - reach).max(0.0));
    dx * dx + dy * dy <= CORNER * CORNER
}

/// Whether (`u`, `v`) is inside the heart: the curve (x² + y² − 1)³ = x²y³, centred, point down.
fn in_heart(u: f32, v: f32) -> bool {
    let x = (u - 0.5) * HEART_SCALE;
    // The curve's middle sits a little above y = 0, so it's moved down to look centred.
    let y = (0.5 - v) * HEART_SCALE + 0.15;
    let r = x * x + y * y - 1.0;
    r * r * r - x * x * y * y * y <= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heart_is_white_on_a_rose_square_with_clear_corners() {
        let pixels = heart_pixels();
        let at = |x: u32, y: u32| &pixels[((y * SIZE + x) * 4) as usize..][..4];
        assert_eq!(at(0, 0)[3], 0, "the corners are rounded off");
        assert_eq!(at(SIZE / 2, SIZE / 2), [255, 255, 255, 255], "the middle is the heart");
        assert_eq!(at(SIZE / 2, 1), [237, 37, 83, 255], "above the heart is the square");
        assert_eq!(at(SIZE / 2, SIZE - 2), [237, 37, 83, 255], "below the heart's point is the square");
    }
}
