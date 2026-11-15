use super::*;

fn renderer() -> MediaRenderer {
    MediaRenderer::new().expect("these tests need a GPU")
}

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    rgba.repeat((width * height) as usize)
}

fn quad(texture: TextureKey, x: f32, y: f32, width: f32, height: f32) -> Quad {
    Quad {
        texture,
        rect: Rect { x, y, width, height },
        opacity: 1.0,
        checker: None,
        filter: Filter::Smooth,
    }
}

fn view(width: u32, height: u32, background: [f32; 3], quads: Vec<Quad>) -> View {
    View {
        width,
        height,
        background,
        quads,
        revision: 7,
    }
}

/// Renders `view` on a fresh target and waits for the frame.
fn render(renderer: &mut MediaRenderer, view: &View) -> Frame {
    let target = renderer.create_target();
    assert!(renderer.submit(target, view).unwrap());
    renderer.wait().unwrap();
    let frame = renderer.take_frame(target).unwrap().expect("a frame after waiting");
    renderer.release_target(target);
    assert_eq!((frame.width, frame.height), (view.width, view.height));
    assert_eq!(frame.bgra.len(), (view.width * view.height * 4) as usize);
    assert_eq!(frame.revision, view.revision);
    frame
}

/// The frame's pixel at (x, y) as BGRA.
fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * frame.width + x) * 4) as usize;
    frame.bgra[offset..offset + 4].try_into().unwrap()
}

fn assert_near(actual: [u8; 4], expected: [u8; 4], tolerance: u8, what: &str) {
    let close = actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= tolerance);
    assert!(close, "{what}: got BGRA {actual:?}, expected {expected:?} (within {tolerance})");
}

fn linear_to_srgb_byte(linear: f32) -> u8 {
    let linear = linear.clamp(0.0, 1.0);
    let encoded = if linear <= 0.0031308 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 };
    (encoded * 255.0).round() as u8
}

fn srgb_byte_to_linear(byte: u8) -> f32 {
    srgb_to_linear(f32::from(byte) / 255.0)
}

#[test]
fn reports_its_adapter() {
    let renderer = renderer();
    eprintln!("adapter: {}, max texture size {}", renderer.adapter_summary(), renderer.max_texture_size());
    assert!(!renderer.adapter_summary().is_empty());
    assert!(renderer.max_texture_size() >= 2048);
}

#[test]
fn renderer_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<MediaRenderer>();
}

#[test]
fn readback_is_tightly_packed_across_row_padding() {
    let mut renderer = renderer();
    // 301 * 4 = 1204 bytes per row, padded to 1280 in the staging buffer.
    let (width, height) = (301, 37);
    let expected = |x: u32, y: u32| [((x + y) % 256) as u8, (y * 6 % 256) as u8, (x % 256) as u8, 255];
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let [b, g, r, a] = expected(x, y);
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    renderer.upload(TextureKey(1), width, height, &rgba, false).unwrap();
    let mut coordinates = quad(TextureKey(1), 0.0, 0.0, width as f32, height as f32);
    coordinates.filter = Filter::Pixelated;
    let frame = render(&mut renderer, &view(width, height, [0.0; 3], vec![coordinates]));
    for y in 0..height {
        for x in 0..width {
            assert_near(pixel(&frame, x, y), expected(x, y), 1, &format!("pixel ({x}, {y})"));
        }
    }
}

#[test]
fn solid_colour_round_trips_through_srgb() {
    let mut renderer = renderer();
    renderer.upload(TextureKey(1), 4, 4, &solid(4, 4, [200, 100, 50, 255]), true).unwrap();
    let frame = render(&mut renderer, &view(64, 48, [0.0; 3], vec![quad(TextureKey(1), 0.0, 0.0, 64.0, 48.0)]));
    for y in 0..48 {
        for x in 0..64 {
            assert_near(pixel(&frame, x, y), [50, 100, 200, 255], 1, &format!("pixel ({x}, {y})"));
        }
    }
}

#[test]
fn half_opacity_blends_in_linear_light() {
    let mut renderer = renderer();
    renderer.upload(TextureKey(1), 2, 2, &solid(2, 2, [255, 0, 0, 255]), false).unwrap();
    let mut red = quad(TextureKey(1), 0.0, 0.0, 32.0, 32.0);
    red.opacity = 0.5;
    let frame = render(&mut renderer, &view(32, 32, [1.0, 1.0, 1.0], vec![red]));
    let half = linear_to_srgb_byte(0.5);
    assert_eq!(half, 188);
    for y in 0..32 {
        for x in 0..32 {
            assert_near(pixel(&frame, x, y), [half, half, 255, 255], 1, &format!("pixel ({x}, {y})"));
        }
    }
}

#[test]
fn checker_shows_through_transparent_pixels() {
    let mut renderer = renderer();
    // Fully transparent red: premultiplied, no red may leak into the picture.
    let mut rgba = solid(8, 8, [255, 0, 0, 0]);
    // The bottom half is half-transparent green.
    for pixel in rgba[8 * 4 * 4..].as_chunks_mut::<4>().0 {
        *pixel = [0, 255, 0, 128];
    }
    renderer.upload(TextureKey(1), 8, 8, &rgba, true).unwrap();
    let mut checkered = quad(TextureKey(1), 0.0, 0.0, 64.0, 64.0);
    checkered.filter = Filter::Pixelated;
    checkered.checker = Some(Checker {
        light: [1.0, 1.0, 1.0],
        dark: [0.0, 0.0, 0.0],
        size: 8.0,
    });
    let frame = render(&mut renderer, &view(64, 64, [1.0, 0.0, 0.0], vec![checkered]));

    let (white, black) = ([255, 255, 255, 255], [0, 0, 0, 255]);
    assert_near(pixel(&frame, 4, 4), white, 1, "light square");
    assert_near(pixel(&frame, 12, 4), black, 1, "dark square");
    assert_near(pixel(&frame, 12, 12), white, 1, "light square");
    assert_near(pixel(&frame, 20, 28), black, 1, "dark square");

    let alpha = 128.0 / 255.0;
    let over_white = [linear_to_srgb_byte(1.0 - alpha), 255, linear_to_srgb_byte(1.0 - alpha), 255];
    let over_black = [0, linear_to_srgb_byte(alpha), 0, 255];
    assert_near(pixel(&frame, 36, 36), over_white, 2, "half-transparent green over a light square");
    assert_near(pixel(&frame, 44, 36), over_black, 2, "half-transparent green over a dark square");
}

#[test]
fn tiles_meet_without_gaps() {
    let mut renderer = MediaRenderer::with_max_texture_size(64).expect("these tests need a GPU");
    assert_eq!(renderer.max_texture_size(), 64);
    let colour = [30, 160, 90, 255];
    renderer.upload(TextureKey(1), 100, 90, &solid(100, 90, colour), true).unwrap();
    assert_eq!(renderer.textures[&TextureKey(1)].tiles.len(), 4);
    assert_eq!(renderer.texture_size(TextureKey(1)), Some((100, 90)));

    // Magnified, minified, and at fractional positions, every pixel inside the image is the image.
    for rect in [
        Rect { x: 3.3, y: 2.7, width: 151.7, height: 133.1 },
        Rect { x: 10.37, y: 5.61, width: 37.0, height: 33.3 },
        Rect { x: -1000.25, y: -900.5, width: 2000.0, height: 1800.0 },
    ] {
        let frame = render(&mut renderer, &view(160, 140, [0.0; 3], vec![quad(TextureKey(1), rect.x, rect.y, rect.width, rect.height)]));
        for y in 0..140 {
            for x in 0..160 {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let inside = cx > rect.x + 1.0 && cx < rect.x + rect.width - 1.0 && cy > rect.y + 1.0 && cy < rect.y + rect.height - 1.0;
                if inside {
                    assert_near(pixel(&frame, x, y), [90, 160, 30, 255], 1, &format!("pixel ({x}, {y}) of {rect:?}"));
                }
            }
        }
    }

    // A horizontal gradient magnified across the vertical seam at x = 64 stays smooth there.
    let mut gradient = Vec::new();
    for _ in 0..90 {
        for x in 0..100u32 {
            let value = (x * 255 / 99) as u8;
            gradient.extend_from_slice(&[value, value, value, 255]);
        }
    }
    renderer.upload(TextureKey(2), 100, 90, &gradient, true).unwrap();
    let frame = render(&mut renderer, &view(200, 180, [1.0, 0.0, 0.0], vec![quad(TextureKey(2), 0.0, 0.0, 200.0, 180.0)]));
    for y in [0, 60, 127, 128, 129, 179] {
        for x in 1..200 {
            let (left, right) = (pixel(&frame, x - 1, y), pixel(&frame, x, y));
            assert_eq!(right[0], right[2], "pixel ({x}, {y}) should be grey, not the background");
            assert!(right[2] >= left[2] && right[2] - left[2] <= 4, "step at ({x}, {y}): {left:?} -> {right:?}");
        }
    }
}

#[test]
fn quads_past_the_target_are_clipped() {
    let mut renderer = renderer();
    renderer.upload(TextureKey(1), 16, 16, &solid(16, 16, [10, 20, 240, 255]), true).unwrap();
    let mut faded = quad(TextureKey(1), 0.0, 0.0, 50.0, 40.0);
    faded.opacity = 0.0;
    let quads = vec![
        quad(TextureKey(1), -500.0, -300.0, 5000.0, 4000.0),
        quad(TextureKey(1), 1e9, 1e9, 10.0, 10.0),
        quad(TextureKey(1), -1e9, -1e9, 2e9, 2e9),
        quad(TextureKey(1), f32::NAN, 0.0, 10.0, 10.0),
        quad(TextureKey(1), 0.0, 0.0, f32::INFINITY, 10.0),
        quad(TextureKey(1), 5.0, 5.0, -10.0, 10.0),
        faded,
    ];
    let frame = render(&mut renderer, &view(50, 40, [0.0; 3], quads));
    for y in 0..40 {
        for x in 0..50 {
            assert_near(pixel(&frame, x, y), [240, 20, 10, 255], 1, &format!("pixel ({x}, {y})"));
        }
    }
}

#[test]
fn uploading_the_same_size_again_reuses_the_texture() {
    let mut renderer = renderer();
    let key = TextureKey(1);
    renderer.upload(key, 32, 32, &solid(32, 32, [255, 0, 0, 255]), true).unwrap();
    let memory = renderer.texture_memory();
    let texture = renderer.textures[&key].tiles[0].texture.clone();
    renderer.upload(key, 32, 32, &solid(32, 32, [0, 0, 255, 255]), true).unwrap();
    assert_eq!(renderer.texture_memory(), memory);
    assert!(renderer.textures[&key].tiles[0].texture == texture, "the GPU texture should be reused");

    let frame = render(&mut renderer, &view(16, 16, [0.0; 3], vec![quad(key, 0.0, 0.0, 16.0, 16.0)]));
    assert_near(pixel(&frame, 8, 8), [255, 0, 0, 255], 1, "the second upload's blue");

    renderer.upload(key, 32, 32, &solid(32, 32, [0, 255, 0, 255]), false).unwrap();
    assert_eq!(renderer.texture_memory(), 32 * 32 * 4, "without mipmaps it's a new, smaller texture");
    renderer.upload(key, 16, 32, &solid(16, 32, [0, 255, 0, 128]), false).unwrap();
    assert_eq!(renderer.texture_memory(), 16 * 32 * 4);
}

#[test]
fn release_frees_texture_memory() {
    let mut renderer = renderer();
    renderer.upload(TextureKey(1), 64, 64, &solid(64, 64, [1, 2, 3, 255]), true).unwrap();
    renderer.upload(TextureKey(2), 100, 50, &solid(100, 50, [1, 2, 3, 255]), false).unwrap();
    let mipmapped = 4 * (4096 + 1024 + 256 + 64 + 16 + 4 + 1);
    assert_eq!(renderer.texture_memory(), mipmapped + 20_000);
    assert!(renderer.contains(TextureKey(1)));

    renderer.release(TextureKey(1));
    assert_eq!(renderer.texture_memory(), 20_000);
    assert!(!renderer.contains(TextureKey(1)));
    assert_eq!(renderer.texture_size(TextureKey(1)), None);
    renderer.release(TextureKey(1));
    renderer.release(TextureKey(2));
    assert_eq!(renderer.texture_memory(), 0);
}

#[test]
fn submit_reports_a_full_ring() {
    let mut renderer = renderer();
    let target = renderer.create_target();
    assert!(!renderer.has_pending_frames(target));
    let mut frame_view = view(32, 32, [0.5; 3], Vec::new());
    for revision in 1..=3 {
        frame_view.revision = revision;
        assert!(renderer.submit(target, &frame_view).unwrap(), "submit {revision}");
    }
    frame_view.revision = 4;
    assert!(!renderer.submit(target, &frame_view).unwrap(), "every staging buffer is in flight");
    assert!(renderer.has_pending_frames(target));

    renderer.wait().unwrap();
    let frame = renderer.take_frame(target).unwrap().expect("a finished frame");
    assert_eq!(frame.revision, 3, "the newest frame wins");
    assert!(!renderer.has_pending_frames(target), "older frames are recycled");
    assert!(renderer.take_frame(target).unwrap().is_none());
    assert!(renderer.submit(target, &frame_view).unwrap());
}

#[test]
fn unknown_textures_are_skipped() {
    let mut renderer = renderer();
    let background = [0.2, 0.4, 0.6];
    let frame = render(&mut renderer, &view(20, 10, background, vec![quad(TextureKey(999), 0.0, 0.0, 20.0, 10.0)]));
    for y in 0..10 {
        for x in 0..20 {
            assert_near(pixel(&frame, x, y), [153, 102, 51, 255], 1, &format!("pixel ({x}, {y})"));
        }
    }
}

#[test]
fn resizing_discards_frames_in_flight() {
    let mut renderer = renderer();
    let target = renderer.create_target();
    let mut frame_view = view(64, 64, [0.0; 3], Vec::new());
    for revision in 1..=2 {
        frame_view.revision = revision;
        assert!(renderer.submit(target, &frame_view).unwrap());
    }
    frame_view.width = 80;
    frame_view.height = 60;
    frame_view.revision = 3;
    assert!(renderer.submit(target, &frame_view).unwrap());
    renderer.wait().unwrap();
    let frame = renderer.take_frame(target).unwrap().expect("the resized frame");
    assert_eq!((frame.width, frame.height, frame.revision), (80, 60, 3));
    assert_eq!(frame.bgra.len(), 80 * 60 * 4);

    // Releasing a target with frames in flight is fine too.
    assert!(renderer.submit(target, &frame_view).unwrap());
    renderer.release_target(target);
    renderer.wait().unwrap();
    assert!(renderer.take_frame(target).is_err(), "the target is gone");
    assert!(!renderer.has_pending_frames(target));
}

#[test]
fn mipmaps_filter_in_linear_light() {
    let mut renderer = renderer();
    let mut rgba = Vec::new();
    for y in 0..256u32 {
        for x in 0..256u32 {
            let value = if (x + y) % 2 == 0 { 255 } else { 0 };
            rgba.extend_from_slice(&[value, value, value, 255]);
        }
    }
    renderer.upload(TextureKey(1), 256, 256, &rgba, true).unwrap();
    let frame = render(&mut renderer, &view(64, 64, [1.0, 0.0, 0.0], vec![quad(TextureKey(1), 0.0, 0.0, 64.0, 64.0)]));
    // Half the light of white is sRGB 188, not the 128 that averaging encoded values would give.
    for y in 0..64 {
        for x in 0..64 {
            assert_near(pixel(&frame, x, y), [188, 188, 188, 255], 2, &format!("pixel ({x}, {y})"));
        }
    }
}

#[test]
fn pixelated_magnifies_with_hard_edges() {
    let mut renderer = renderer();
    renderer.upload(TextureKey(1), 2, 1, &[0, 0, 0, 255, 255, 255, 255, 255], true).unwrap();
    let mut sharp = quad(TextureKey(1), 0.0, 0.0, 64.0, 8.0);
    sharp.filter = Filter::Pixelated;
    let frame = render(&mut renderer, &view(64, 8, [1.0, 0.0, 0.0], vec![sharp]));
    assert_near(pixel(&frame, 31, 4), [0, 0, 0, 255], 0, "last black pixel");
    assert_near(pixel(&frame, 32, 4), [255, 255, 255, 255], 0, "first white pixel");

    let frame = render(&mut renderer, &view(64, 8, [1.0, 0.0, 0.0], vec![quad(TextureKey(1), 0.0, 0.0, 64.0, 8.0)]));
    let middle = pixel(&frame, 31, 4);
    assert!(middle[0] > 100 && middle[0] < 230, "smooth filtering blends at the middle: {middle:?}");
}

#[test]
fn upload_rejects_mismatched_pixels() {
    let mut renderer = renderer();
    assert!(renderer.upload(TextureKey(1), 4, 4, &[0; 60], false).is_err());
    assert!(renderer.upload(TextureKey(1), 0, 4, &[], false).is_err());
    assert!(!renderer.contains(TextureKey(1)));
    assert!(renderer.submit(TargetId(12345), &view(4, 4, [0.0; 3], Vec::new())).is_err());
}

#[test]
fn background_converts_from_srgb() {
    assert!((srgb_byte_to_linear(188) - 0.5).abs() < 0.003);
    let mut renderer = renderer();
    let frame = render(&mut renderer, &view(3, 3, [0.5, 0.25, 1.0], Vec::new()));
    assert_near(pixel(&frame, 1, 1), [255, 64, 128, 255], 1, "background");
}
