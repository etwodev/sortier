//! Exercises the public API on files the tests make themselves: H.264 MPEG-4 videos written with OpenH264 and the
//! `mp4` writer, animations written with `image` and `png`, stills, and documents (PDFs, comic archives and EPUBs)
//! written by hand. WebM and Matroska clips are made with the
//! `ffmpeg` program where it's installed (only as a way to write them: Sortier itself doesn't need it), and those
//! tests say so when it isn't.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mp4::{AvcConfig, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType};
use openh264::OpenH264API;
use openh264::encoder::{Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode, VuiConfig};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use sortier_media::{
    MediaError, Picture, VideoPlayer, decode_animation, decode_still, detect_hardware_decoder, document_page,
    document_page_count, document_page_size, image_size, probe_video, video_thumbnail,
};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const FPS: f64 = 12.0;
const FRAMES: usize = 24;

/// Frame `index`'s flat colour, changing clearly from frame to frame so each can be told apart despite lossy
/// compression.
const PALETTE: [[u8; 3]; 6] = [[30, 200, 90], [220, 40, 60], [40, 60, 220], [240, 220, 30], [20, 20, 20], [250, 250, 250]];

fn shade(index: usize) -> [u8; 3] {
    PALETTE[index % PALETTE.len()]
}

/// The palette entry nearest the picture's middle pixel.
fn shade_of(picture: &Picture) -> usize {
    let middle = ((picture.height / 2 * picture.width + picture.width / 2) * 4) as usize;
    let pixel = &picture.rgba[middle..middle + 3];
    let distance = |colour: &[u8; 3]| (0..3).map(|channel| (i32::from(pixel[channel]) - i32::from(colour[channel])).pow(2)).sum::<i32>();
    (0..PALETTE.len()).min_by_key(|&index| distance(&PALETTE[index])).unwrap()
}

/// Writes `frames` flat-coloured frames of the given size as H.264 in an MPEG-4 file at [`FPS`], with a keyframe every six frames,
/// as Spline's built-in encoder does.
fn write_mp4(path: &Path, frames: usize, (width, height): (u32, u32)) {
    let config = EncoderConfig::new()
        .max_frame_rate(FrameRate::from_hz(FPS as f32))
        .rate_control_mode(RateControlMode::Off)
        .intra_frame_period(IntraFramePeriod::from_num_frames(6))
        .vui(VuiConfig::bt601());
    let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config).unwrap();
    let timescale = 90_000_u32;
    let frame_duration = (f64::from(timescale) / FPS).round() as u32;
    let mut file = Some(BufWriter::new(File::create(path).unwrap()));
    let mut writer: Option<Mp4Writer<BufWriter<File>>> = None;
    for index in 0..frames {
        let [red, green, blue] = shade(index);
        let rgb: Vec<u8> = (0..width * height).flat_map(|_| [red, green, blue]).collect();
        let yuv = YUVBuffer::from_rgb8_source(RgbSliceU8::new(&rgb, (width as usize, height as usize)));
        let stream = encoder.encode(&yuv).unwrap();
        let is_sync = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        let annex_b = stream.to_vec();
        let (mut sample, mut sequence, mut picture) = (Vec::new(), None, None);
        for unit in openh264::nal_units(&annex_b) {
            let zeros = unit.iter().take_while(|&&byte| byte == 0).count();
            let unit = if zeros >= 2 && unit.get(zeros) == Some(&1) { &unit[zeros + 1..] } else { unit };
            match unit.first().map(|header| header & 0x1f) {
                Some(7) => sequence = Some(unit.to_vec()),
                Some(8) => picture = Some(unit.to_vec()),
                Some(_) => {
                    sample.extend_from_slice(&(unit.len() as u32).to_be_bytes());
                    sample.extend_from_slice(unit);
                }
                None => {}
            }
        }
        if writer.is_none() {
            let config = Mp4Config {
                major_brand: "isom".parse().unwrap(),
                minor_version: 512,
                compatible_brands: ["isom", "iso2", "avc1", "mp41"].iter().map(|brand| brand.parse().unwrap()).collect(),
                timescale: 1000,
            };
            let mut started = Mp4Writer::write_start(file.take().unwrap(), &config).unwrap();
            started
                .add_track(&TrackConfig {
                    track_type: TrackType::Video,
                    timescale,
                    language: "und".into(),
                    media_conf: MediaConfig::AvcConfig(AvcConfig {
                        width: width as u16,
                        height: height as u16,
                        seq_param_set: sequence.expect("the first frame carries the parameter sets"),
                        pic_param_set: picture.expect("the first frame carries the parameter sets"),
                    }),
                })
                .unwrap();
            writer = Some(started);
        }
        writer
            .as_mut()
            .unwrap()
            .write_sample(
                1,
                &Mp4Sample {
                    start_time: index as u64 * u64::from(frame_duration),
                    duration: frame_duration,
                    rendering_offset: 0,
                    is_sync,
                    bytes: sample.into(),
                },
            )
            .unwrap();
    }
    writer.unwrap().write_end().unwrap();
}

fn video() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("colours.mp4");
    write_mp4(&path, FRAMES, (WIDTH, HEIGHT));
    (dir, path)
}

/// Polls `check` every few milliseconds until it gives something, failing after `seconds`.
fn wait_for<T>(seconds: f64, what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn frame_index(time: Duration) -> usize {
    (time.as_secs_f64() * FPS + 1e-6).floor() as usize
}

#[test]
fn videos_are_probed_and_thumbnailed() {
    let (_dir, path) = video();
    let info = probe_video(&path).unwrap();
    assert_eq!((info.width, info.height), (WIDTH, HEIGHT));
    assert!((info.duration.as_secs_f64() - FRAMES as f64 / FPS).abs() < 0.01, "{:?}", info.duration);
    assert!((info.frame_rate - FPS).abs() < 0.01, "{}", info.frame_rate);
    assert!(!info.has_sound);

    let thumbnail = video_thumbnail(&path, 32).unwrap();
    assert_eq!((thumbnail.width, thumbnail.height), (32, 24));
    assert_eq!(thumbnail.rgba.len(), 32 * 24 * 4);
    // A tenth of the way into two seconds is 0.2 s, which is frame 2 at 12 fps.
    assert_eq!(shade_of(&thumbnail), 2);
    let full = video_thumbnail(&path, 1000).unwrap();
    assert_eq!((full.width, full.height), (WIDTH, HEIGHT), "never enlarged");

    let dir = tempfile::tempdir().unwrap();
    let short = dir.path().join("short.mp4");
    write_mp4(&short, 6, (WIDTH, HEIGHT));
    assert_eq!(shade_of(&video_thumbnail(&short, 64).unwrap()), 0, "the first frame of a half-second clip");
}

#[test]
fn seeking_while_paused_shows_the_frame_at_the_new_time() {
    let (_dir, path) = video();
    let player = VideoPlayer::open(&path, None).unwrap();
    assert!(!player.is_playing());
    let first = player.current_frame().expect("the first frame is decoded on opening");
    assert_eq!((first.picture.width, first.picture.height), (WIDTH, HEIGHT));
    assert_eq!(first.time, Duration::ZERO);
    assert_eq!(shade_of(&first.picture), 0);

    for (seconds, frame) in [(1.25, 15), (0.34, 4), (1.95, 23), (0.0, 0), (0.6, 7)] {
        let serial = player.frame_serial();
        player.seek(Duration::from_secs_f64(seconds));
        let shown = wait_for(5.0, &format!("frame {frame} after seeking to {seconds} s"), || {
            player.current_frame().filter(|shown| frame_index(shown.time) == frame)
        });
        assert_eq!(shade_of(&shown.picture), frame % PALETTE.len(), "frame {frame}");
        assert!(player.frame_serial() > serial);
        assert!((player.position().as_secs_f64() - seconds).abs() < 1e-6, "paused players stay put");
        assert!(!player.is_playing() && !player.ended());
    }
    let serial = player.frame_serial();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(player.frame_serial(), serial, "nothing changes while paused");
}

#[test]
fn playing_shows_each_frame_at_its_time() {
    let (_dir, path) = video();
    let player = VideoPlayer::open(&path, None).unwrap();
    player.seek(Duration::from_millis(250));
    wait_for(5.0, "the frame at 0.25 s", || player.current_frame().filter(|shown| frame_index(shown.time) == 3));
    let started = Instant::now();
    player.play();
    assert!(player.is_playing());
    let mut seen = std::collections::BTreeSet::new();
    while started.elapsed() < Duration::from_millis(900) {
        let position = player.position().as_secs_f64();
        let shown = player.current_frame().unwrap();
        let time = shown.time.as_secs_f64();
        let index = frame_index(shown.time);
        assert_eq!(shade_of(&shown.picture), index % PALETTE.len(), "frame {index}");
        assert!(time <= position + 0.02, "frame at {time} s shown at {position} s");
        if started.elapsed() > Duration::from_millis(150) {
            assert!(position - time < 0.25, "frame at {time} s still shown at {position} s");
        }
        seen.insert(index);
        std::thread::sleep(Duration::from_millis(8));
    }
    let position = player.position().as_secs_f64();
    assert!((position - (0.25 + started.elapsed().as_secs_f64())).abs() < 0.1, "the wall clock without sound: {position}");
    assert!(seen.len() >= 8, "most frames of 0.25..1.15 s showed: {seen:?}");

    player.pause();
    let paused = player.position();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(player.position(), paused);
    assert!(!player.is_playing());
}

#[test]
fn playing_stops_at_the_end_or_loops() {
    let (_dir, path) = video();
    let player = VideoPlayer::open(&path, None).unwrap();
    player.seek(Duration::from_millis(1800));
    player.play();
    wait_for(3.0, "the end", || player.ended().then_some(()));
    assert!(!player.is_playing());
    assert_eq!(player.position(), player.info().duration);
    let last = wait_for(3.0, "the last frame", || player.current_frame().filter(|shown| frame_index(shown.time) == FRAMES - 1));
    assert_eq!(shade_of(&last.picture), (FRAMES - 1) % PALETTE.len());

    player.set_looping(true);
    player.play();
    assert!(!player.ended(), "playing again starts over");
    wait_for(3.0, "playing from the start", || (player.position() < Duration::from_millis(500)).then_some(()));
    player.seek(Duration::from_millis(1900));
    wait_for(3.0, "looping back to the start", || (player.position() < Duration::from_millis(1000)).then_some(()));
    assert!(player.is_playing() && !player.ended());
    wait_for(3.0, "the first frames again", || player.current_frame().filter(|shown| frame_index(shown.time) < 6));
}

#[test]
fn players_close_promptly() {
    fn shareable<T: Send + Sync>() {}
    shareable::<VideoPlayer>();
    let (_dir, path) = video();
    let player = VideoPlayer::open(&path, None).unwrap();
    player.play();
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    drop(player);
    assert!(started.elapsed() < Duration::from_millis(100), "dropping never waits on the threads");
}

#[test]
fn stills_decode_and_their_size_reads_from_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let mut image = image::RgbaImage::new(5, 3);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = image::Rgba([x as u8 * 40, y as u8 * 80, 7, 200]);
    }
    let png = dir.path().join("still.png");
    image.save(&png).unwrap();
    let decoded = decode_still(&png).unwrap();
    assert_eq!((decoded.width, decoded.height), (5, 3));
    assert_eq!(decoded.rgba, image.as_raw().clone(), "lossless and straight alpha");
    assert_eq!(image_size(&png).unwrap(), (5, 3));

    // Named wrongly on purpose: the contents decide.
    let jpeg = dir.path().join("photo.png");
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(40, 30, image::Rgba([200, 30, 30, 255])))
        .into_rgb8()
        .save_with_format(&jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    assert_eq!(image_size(&jpeg).unwrap(), (40, 30));
    let photo = decode_still(&jpeg).unwrap();
    assert_eq!(photo.rgba.len(), 40 * 30 * 4);
    assert!(photo.rgba[0] > 150 && photo.rgba[1] < 80 && photo.rgba[3] == 255);

    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "not an image").unwrap();
    assert!(decode_still(&text).is_err());
    assert!(matches!(decode_still(&dir.path().join("missing.png")), Err(MediaError::Io(_))));
}

fn solid(colour: [u8; 3]) -> image::RgbaImage {
    image::RgbaImage::from_pixel(8, 6, image::Rgba([colour[0], colour[1], colour[2], 255]))
}

#[test]
fn animated_gifs_decode_frame_by_frame() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moving.gif");
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(File::create(&path).unwrap());
        encoder.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
        for (index, milliseconds) in [(0, 50), (1, 200), (2, 0)] {
            let delay = image::Delay::from_numer_denom_ms(milliseconds, 1);
            encoder.encode_frame(image::Frame::from_parts(solid(shade(index)), 0, 0, delay)).unwrap();
        }
    }
    let animation = decode_animation(&path).unwrap().expect("three frames");
    assert_eq!((animation.width, animation.height), (8, 6));
    assert_eq!(animation.frames.len(), 3);
    let delays: Vec<u128> = animation.frames.iter().map(|frame| frame.delay.as_millis()).collect();
    assert_eq!(delays, [50, 200, 100], "no delay shows for a tenth of a second");
    for (index, frame) in animation.frames.iter().enumerate() {
        assert_eq!((frame.picture.width, frame.picture.height), (8, 6));
        let pixel = &frame.picture.rgba[..3];
        let want = shade(index);
        assert!((0..3).all(|channel| (i32::from(pixel[channel]) - i32::from(want[channel])).abs() <= 12), "frame {index}: {pixel:?}");
    }
    let still = decode_still(&path).unwrap();
    assert_eq!((still.width, still.height), (8, 6), "the first frame as a still");

    let single = dir.path().join("single.gif");
    solid(shade(1)).save(&single).unwrap();
    assert!(decode_animation(&single).unwrap().is_none());
}

#[test]
fn animated_pngs_decode_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moving.png");
    {
        let mut encoder = png::Encoder::new(BufWriter::new(File::create(&path).unwrap()), 8, 6);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(3, 0).unwrap();
        let mut writer = encoder.write_header().unwrap();
        for (index, hundredths) in [(0, 5_u16), (1, 10), (2, 25)] {
            writer.set_frame_delay(hundredths, 100).unwrap();
            writer.write_image_data(solid(shade(index)).as_raw()).unwrap();
        }
        writer.finish().unwrap();
    }
    let animation = decode_animation(&path).unwrap().expect("three frames");
    assert_eq!(animation.frames.len(), 3);
    let delays: Vec<u128> = animation.frames.iter().map(|frame| frame.delay.as_millis()).collect();
    assert_eq!(delays, [50, 100, 250]);
    for (index, frame) in animation.frames.iter().enumerate() {
        assert_eq!(frame.picture.rgba, solid(shade(index)).into_raw(), "frame {index}");
    }

    let plain = dir.path().join("plain.png");
    solid(shade(0)).save(&plain).unwrap();
    assert!(decode_animation(&plain).unwrap().is_none());
    let jpeg = dir.path().join("photo.jpg");
    image::DynamicImage::ImageRgba8(solid(shade(0))).into_rgb8().save(&jpeg).unwrap();
    assert!(decode_animation(&jpeg).unwrap().is_none(), "JPEGs don't animate");
}

#[test]
fn seeking_with_the_graphics_card_shows_the_same_frames() {
    let Some(hardware) = detect_hardware_decoder() else {
        eprintln!("seeking_with_the_graphics_card_shows_the_same_frames: no hardware decoder here, so this proves nothing");
        return;
    };
    // VideoToolbox won't take H.264 as small as the other tests' clips (those fall back to the processor).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("colours.mp4");
    write_mp4(&path, FRAMES, (320, 240));
    let player = VideoPlayer::open(&path, Some(hardware)).unwrap();
    assert_eq!(player.hardware_decoder(), Some(hardware), "H.264 decodes on the card");
    for (seconds, frame) in [(1.25, 15), (0.34, 4), (0.0, 0)] {
        player.seek(Duration::from_secs_f64(seconds));
        let shown = wait_for(5.0, &format!("frame {frame} after seeking to {seconds} s"), || {
            player.current_frame().filter(|shown| frame_index(shown.time) == frame)
        });
        assert_eq!(shade_of(&shown.picture), frame % PALETTE.len(), "frame {frame}");
    }
    assert!(player.error().is_none(), "{:?}", player.error());
}

#[test]
fn files_that_are_not_videos_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    // The start of an EBML header, as WebM and Matroska files begin, and nothing more.
    let webm = dir.path().join("clip.webm");
    std::fs::write(&webm, [0x1a, 0x45, 0xdf, 0xa3, 0x9f, 0x42, 0x86, 0x81, 0x01]).unwrap();
    let junk = dir.path().join("junk.mp4");
    std::fs::write(&junk, "not a video at all, just some words").unwrap();
    for path in [&webm, &junk] {
        assert!(matches!(probe_video(path), Err(MediaError::Unsupported(_) | MediaError::Decode(_))), "{}", path.display());
        assert!(VideoPlayer::open(path, None).is_err());
        assert!(video_thumbnail(path, 64).is_err());
    }
    assert!(matches!(VideoPlayer::open(&dir.path().join("missing.mp4"), None), Err(MediaError::Io(_))));
}

/// Makes a two-second 320 x 240 clip at `path` with the `ffmpeg` program, red for the first second and blue for the second,
/// with a tone, in the given codecs. `false` when there's no ffmpeg (or it lacks the codecs) to make it with.
fn make_clip(path: &Path, video_codec: &str, sound_codec: &str) -> bool {
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "color=c=red:size=320x240:rate=10:duration=1"])
        .args(["-f", "lavfi", "-i", "color=c=blue:size=320x240:rate=10:duration=1"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
        .args(["-filter_complex", "[0:v][1:v]concat=n=2:v=1,format=yuv420p[v]", "-map", "[v]", "-map", "2:a"])
        .args(["-c:v", video_codec, "-c:a", sound_codec, "-shortest"])
        .arg(path)
        .status();
    matches!(made, Ok(status) if status.success())
}

/// Formats Apple's own frameworks can't read (WebM, VP8, Matroska), each through every entry point, decoded on the
/// processor and with the graphics card decoder when there is one.
#[test]
fn other_containers_and_codecs_probe_thumbnail_and_play() {
    let dir = tempfile::tempdir().unwrap();
    // Whether the graphics card decoder takes each codec on every system it's found on.
    let clips = [
        ("vp9.webm", "libvpx-vp9", "libopus", true),
        ("vp8.webm", "libvpx", "libopus", false),
        ("hevc.mkv", "libx265", "aac", true),
    ];
    for (name, video_codec, sound_codec, on_card) in clips {
        let path = dir.path().join(name);
        if !make_clip(&path, video_codec, sound_codec) {
            eprintln!("other_containers_and_codecs_probe_thumbnail_and_play: couldn't make {name} with ffmpeg, so it wasn't checked");
            continue;
        }
        let info = probe_video(&path).unwrap();
        assert_eq!((info.width, info.height), (320, 240), "{name}");
        assert!((info.duration.as_secs_f64() - 2.0).abs() < 0.15, "{name}: {:?}", info.duration);
        assert!(info.has_sound, "{name}");
        let thumbnail = video_thumbnail(&path, 32).unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (32, 24), "{name}");
        assert!(thumbnail.rgba[0] > 150 && thumbnail.rgba[2] < 100, "{name}: red at 0.2 s");

        for hardware in [None, detect_hardware_decoder()] {
            let player = VideoPlayer::open(&path, hardware).unwrap();
            assert!(player.info().has_sound, "{name}");
            player.seek(Duration::from_millis(1500));
            let shown = wait_for(10.0, "the frame at 1.5 s", || {
                player.current_frame().filter(|shown| shown.time.as_secs_f64() >= 1.4)
            });
            assert!(shown.picture.rgba[2] > 150 && shown.picture.rgba[0] < 100, "{name} ({hardware:?}): blue in the second half");
            player.seek(Duration::ZERO);
            player.play();
            wait_for(10.0, "playing on", || (player.position() > Duration::from_millis(300)).then_some(()));
            let shown = player.current_frame().unwrap();
            assert!(shown.time < Duration::from_millis(1000) && shown.picture.rgba[0] > 150, "{name} ({hardware:?}): red in the first half");
            assert!(player.error().is_none(), "{name} ({hardware:?}): {:?}", player.error());
            if on_card {
                assert_eq!(player.hardware_decoder(), hardware, "{name}");
            }
        }
    }
}

/// A small PNG of one flat colour.
fn png_bytes(colour: [u8; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(solid(colour))
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// A ZIP archive at `path` holding `files` (name, contents), stored uncompressed.
fn write_zip(path: &Path, files: &[(&str, Vec<u8>)]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in files {
        zip.start_file(*name, options).unwrap();
        std::io::Write::write_all(&mut zip, contents).unwrap();
    }
    zip.finish().unwrap();
}

/// The palette entry each page of the document at `path` shows.
fn document_shades(path: &Path) -> Vec<usize> {
    let pages = document_page_count(path).unwrap();
    (0..pages).map(|index| shade_of(&document_page(path, index, None).unwrap())).collect()
}

#[test]
fn comic_archives_read_their_pictures_in_name_order() {
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("volume.cbz");
    write_zip(
        &cbz,
        &[
            ("vol/p10.png", png_bytes(shade(2))),
            ("vol/p2.png", png_bytes(shade(1))),
            ("vol/p1.png", png_bytes(shade(0))),
            ("__MACOSX/vol/._p1.png", b"resource fork".to_vec()),
            ("vol/ComicInfo.xml", b"<ComicInfo/>".to_vec()),
        ],
    );
    assert_eq!(document_shades(&cbz), vec![0, 1, 2], "p1, p2, p10");
    assert_eq!(document_page_size(&cbz, 0).unwrap(), (8, 6));
    let small = document_page(&cbz, 1, Some(4)).unwrap();
    assert_eq!((small.width, small.height), (4, 3));
    assert!(document_page(&cbz, 3, None).is_err(), "there's no fourth page");

    let pictures = dir.path().join("pictures");
    std::fs::create_dir(&pictures).unwrap();
    for (name, index) in [("b-2.png", 1), ("b-10.png", 2), ("b-1.png", 0)] {
        std::fs::write(pictures.join(name), png_bytes(shade(index))).unwrap();
    }
    let cb7 = dir.path().join("volume.cb7");
    sevenz_rust2::compress_to_path(&pictures, &cb7).unwrap();
    assert_eq!(document_shades(&cb7), vec![0, 1, 2]);
}

#[test]
fn picture_epubs_read_in_spine_order_and_text_ones_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let container = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
        <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let package = |spine: &str| {
        format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><manifest>
            <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
            <item id="one" href="text/one.xhtml" media-type="application/xhtml+xml"/>
            <item id="two" href="text/two%20b.xhtml" media-type="application/xhtml+xml"/>
            <item id="three" href="images/three.png" media-type="image/png"/>
            <item id="words" href="text/words.xhtml" media-type="application/xhtml+xml"/>
            </manifest><spine>{spine}</spine></package>"#
        )
        .into_bytes()
    };
    let page = |source: &str| format!(r#"<html><body><img alt="" src="{source}"/></body></html>"#).into_bytes();
    let words = b"<html><body><p>It was a dark and stormy night.</p></body></html>".to_vec();
    let mut files = vec![
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", container.to_vec()),
        ("OEBPS/text/one.xhtml", page("../images/one.png")),
        ("OEBPS/text/two b.xhtml", page("../images/two.png")),
        ("OEBPS/text/words.xhtml", words),
        ("OEBPS/nav.xhtml", b"<html><body><nav><ol><li>One</li></ol></nav></body></html>".to_vec()),
        ("OEBPS/images/one.png", png_bytes(shade(3))),
        ("OEBPS/images/two.png", png_bytes(shade(4))),
        ("OEBPS/images/three.png", png_bytes(shade(5))),
    ];
    let comic = dir.path().join("comic.epub");
    files.push((
        "OEBPS/content.opf",
        package(r#"<itemref idref="nav" linear="no"/><itemref idref="two"/><itemref idref="one"/><itemref idref="three"/>"#),
    ));
    write_zip(&comic, &files);
    assert_eq!(document_shades(&comic), vec![4, 3, 5], "the spine's order, skipping what isn't read in order");

    let novel = dir.path().join("novel.epub");
    files.pop();
    files.push((
        "OEBPS/content.opf",
        package(r#"<itemref idref="one"/><itemref idref="words"/><itemref idref="words"/><itemref idref="words"/>"#),
    ));
    write_zip(&novel, &files);
    assert!(matches!(document_page_count(&novel), Err(MediaError::Unsupported(_))));
}

/// A one-page PDF, 200 x 100 points, filled red on its left half.
fn write_pdf(path: &Path) {
    let content = "1 0 0 rg 0 0 100 100 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    std::fs::write(path, pdf).unwrap();
}

#[test]
fn pdf_pages_are_drawn_at_twice_their_size_on_white() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.pdf");
    write_pdf(&path);
    assert_eq!(document_page_count(&path).unwrap(), 1);
    assert_eq!(document_page_size(&path, 0).unwrap(), (400, 200));
    let page = document_page(&path, 0, None).unwrap();
    assert_eq!((page.width, page.height), (400, 200));
    let pixel = |x: u32, y: u32| {
        let at = ((y * page.width + x) * 4) as usize;
        page.rgba[at..at + 4].to_vec()
    };
    assert_eq!(pixel(50, 100), vec![255, 0, 0, 255], "red on the left");
    assert_eq!(pixel(350, 100), vec![255, 255, 255, 255], "white paper on the right");
    let thumbnail = document_page(&path, 0, Some(100)).unwrap();
    assert_eq!((thumbnail.width, thumbnail.height), (100, 50));

    let broken = dir.path().join("broken.pdf");
    std::fs::write(&broken, b"%PDF-1.4\nnot really").unwrap();
    assert!(document_page_count(&broken).is_err());
}

#[test]
fn videos_play_from_a_stream_as_from_a_file() {
    let (_dir, path) = video();
    let reading = path.clone();
    let open: sortier_media::OpenStream = std::sync::Arc::new(move || {
        let file = File::open(&reading)?;
        Ok(Box::new(file) as Box<dyn sortier_media::ReadSeek + Send>)
    });
    let source = sortier_media::VideoSource::Stream {
        name: "colours.mp4".into(),
        open,
    };
    let player = VideoPlayer::open_source(&source, None).unwrap();
    assert_eq!((player.info().width, player.info().height), (WIDTH, HEIGHT));
    player.seek(Duration::from_secs_f64(1.25));
    let shown = wait_for(5.0, "frame 15", || player.current_frame().filter(|shown| frame_index(shown.time) == 15));
    assert_eq!(shade_of(&shown.picture), 15 % PALETTE.len());
}
