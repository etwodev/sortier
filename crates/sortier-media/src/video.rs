//! Reading videos with FFmpeg's libraries, linked into Sortier: what a video is, and its pictures from any point,
//! decoded on the graphics card when asked and able.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::ptr;
use std::sync::Once;

use ffmpeg_next::codec::threading;
use ffmpeg_next::decoder::Opened;
use ffmpeg_next::format::context::Input;
use ffmpeg_next::format::stream::Disposition;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::scaling;
use ffmpeg_next::util::color;
use ffmpeg_next::util::format::Pixel;
use ffmpeg_next::util::frame::{self, Frame};
use ffmpeg_next::{Dictionary, Error, Packet, Rational, codec, ffi};

use crate::hardware::{HardwareDecoder, detect_hardware_decoder};
use crate::{MediaError, Picture, VideoInfo, fit_size};

/// Frame rates outside this range are taken to be mistakes in the file.
fn plausible_rate(rate: f64) -> f64 {
    if (1.0..=240.0).contains(&rate) { rate } else { 30.0 }
}

pub(crate) fn seconds_per_tick(rate: Rational) -> f64 {
    if rate.denominator() == 0 { 0.0 } else { f64::from(rate.numerator()) / f64::from(rate.denominator()) }
}

/// FFmpeg's error as one of ours.
pub(crate) fn media_error(error: Error) -> MediaError {
    match error {
        Error::InvalidData | Error::DemuxerNotFound => MediaError::Unsupported("the file isn't a video Sortier can read".into()),
        Error::DecoderNotFound => MediaError::Unsupported("Sortier can't decode this video's format".into()),
        other => MediaError::Decode(other.to_string()),
    }
}

/// Something both readable and seekable, such as a file on a server read a block at a time.
pub trait ReadSeek: Read + Seek {}

impl<T: Read + Seek> ReadSeek for T {}

/// Opens a new reader of a video. Pictures and sound are read separately, each with its own.
pub type OpenStream = Arc<dyn Fn() -> std::io::Result<Box<dyn ReadSeek + Send>> + Send + Sync>;

/// Where a video is read from.
#[derive(Clone)]
pub enum VideoSource {
    File(PathBuf),
    /// Readers opened by `open`, of a file called `name` (which helps tell its format).
    Stream { name: String, open: OpenStream },
}

/// Quietens FFmpeg's own logging the first time; errors come back as values.
fn quieten_ffmpeg() {
    static QUIET: Once = Once::new();
    QUIET.call_once(|| ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Fatal));
}

/// Opens `source` for reading.
pub(crate) fn open_source(source: &VideoSource) -> Result<Input, MediaError> {
    match source {
        VideoSource::File(path) => open_input(path),
        VideoSource::Stream { name, open } => {
            quieten_ffmpeg();
            let io = ffmpeg_next::format::context::StreamIo::from_read_seek(open()?).map_err(media_error)?;
            // Nothing in a streamed file (a playlist, say) may make FFmpeg open anything else.
            let mut options = Dictionary::new();
            options.set("protocol_whitelist", "none");
            ffmpeg_next::format::input_from_stream(io, Some(name), Some(options)).map_err(media_error)
        }
    }
}

/// Opens `path` for reading.
///
/// Only local files are read: the path is given as a `file:` URL, so names with colons aren't taken for other
/// protocols, and playlists inside a file can't make FFmpeg reach the network.
pub(crate) fn open_input(path: &Path) -> Result<Input, MediaError> {
    quieten_ffmpeg();
    let text = path
        .to_str()
        .ok_or_else(|| MediaError::Unsupported("the file's name isn't valid Unicode".into()))?;
    if !path.is_file() {
        return Err(MediaError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)));
    }
    let mut options = Dictionary::new();
    options.set("protocol_whitelist", "file");
    ffmpeg_next::format::input_with_dictionary(&format!("file:{text}"), options).map_err(|error| match error {
        Error::Other { errno } => MediaError::Io(std::io::Error::from_raw_os_error(errno)),
        other => media_error(other),
    })
}

/// One stream of an open file, read packet by packet into a decoder, with its times in seconds from the start of
/// the file's timeline (so every stream counts from the same zero).
pub(crate) struct StreamReader {
    pub(crate) input: Input,
    pub(crate) index: usize,
    seconds_per_tick: f64,
    origin: f64,
    ended: bool,
}

impl StreamReader {
    /// Reads stream `index` of `input`, telling the demuxer to skip every other stream.
    pub(crate) fn new(mut input: Input, index: usize) -> Self {
        // SAFETY: the context is open, and `streams` holds `nb_streams` valid stream pointers.
        let start = unsafe {
            let context = input.as_mut_ptr();
            for other in 0..(*context).nb_streams as usize {
                if other != index {
                    (**(*context).streams.add(other)).discard = ffi::AVDiscard::AVDISCARD_ALL;
                }
            }
            (*context).start_time
        };
        let time_base = input.stream(index).expect("the index came from this input").time_base();
        Self {
            input,
            index,
            seconds_per_tick: seconds_per_tick(time_base),
            origin: if start == ffi::AV_NOPTS_VALUE { 0.0 } else { start as f64 / f64::from(ffi::AV_TIME_BASE) },
            ended: false,
        }
    }

    /// A timestamp of the stream in seconds.
    pub(crate) fn seconds(&self, ticks: i64) -> f64 {
        ticks as f64 * self.seconds_per_tick - self.origin
    }

    /// Moves to the last keyframe at or before `seconds`. The decoder reading from here must be flushed.
    pub(crate) fn seek(&mut self, seconds: f64) -> Result<(), MediaError> {
        let target = ((seconds + self.origin) * f64::from(ffi::AV_TIME_BASE)) as i64;
        self.input.seek(target, ..target).or_else(|_| self.input.seek(target, ..)).map_err(media_error)?;
        self.ended = false;
        Ok(())
    }

    /// Decodes the next frame into `frame`, feeding `decoder` packets as it asks. `false` once it has given every
    /// frame.
    pub(crate) fn decode(&mut self, decoder: &mut Opened, frame: &mut Frame) -> Result<bool, MediaError> {
        loop {
            match decoder.receive_frame(frame) {
                Ok(()) => return Ok(true),
                Err(Error::Eof) => return Ok(false),
                Err(Error::Other { errno }) if errno == ffmpeg_next::util::error::EAGAIN => self.feed(decoder)?,
                Err(error) => return Err(media_error(error)),
            }
        }
    }

    /// Gives `decoder` the stream's next packet, or tells it the stream has ended. Damaged packets are skipped, as
    /// players do.
    fn feed(&mut self, decoder: &mut Opened) -> Result<(), MediaError> {
        if self.ended {
            return Err(MediaError::Decode("the decoder wanted more after the end".into()));
        }
        loop {
            let mut packet = Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() != self.index => {}
                Ok(()) => match decoder.send_packet(&packet) {
                    Ok(()) | Err(Error::InvalidData) => return Ok(()),
                    Err(error) => return Err(media_error(error)),
                },
                Err(Error::InvalidData) => {}
                Err(Error::Eof) => {
                    self.ended = true;
                    return decoder.send_eof().map_err(media_error);
                }
                Err(error) => return Err(media_error(error)),
            }
        }
    }
}

/// The file's first video stream that isn't cover art.
fn video_stream(input: &Input) -> Result<usize, MediaError> {
    input
        .streams()
        .find(|stream| stream.parameters().medium() == Type::Video && !stream.disposition().contains(Disposition::ATTACHED_PIC))
        .map(|stream| stream.index())
        .ok_or_else(|| MediaError::Unsupported("the file has no video".into()))
}

/// The file's main sound stream, if it has one Sortier can decode.
pub(crate) fn sound_stream(input: &Input) -> Option<usize> {
    let stream = input.streams().best(Type::Audio)?;
    ffmpeg_next::decoder::find(stream.parameters().id()).map(|_| stream.index())
}

/// The stream's codec parameters as FFmpeg keeps them.
fn raw_parameters(input: &Input, index: usize) -> *const ffi::AVCodecParameters {
    let stream = input.stream(index).expect("the index came from this input");
    // SAFETY: the stream belongs to the open context, which owns its parameters.
    unsafe { (*stream.as_ptr()).codecpar }
}

/// Quarter turns clockwise the stream asks for when showing its pictures (phones record this way); none for
/// anything but a plain rotation.
fn turns_of(input: &Input, index: usize) -> u8 {
    let parameters = raw_parameters(input, index);
    // SAFETY: the side data array and its count come from the same parameters, and a display matrix is nine
    // 32-bit values, which the size check confirms before it's read.
    let degrees = unsafe {
        let side = ffi::av_packet_side_data_get(
            (*parameters).coded_side_data,
            (*parameters).nb_coded_side_data,
            ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
        );
        if side.is_null() || (*side).size < 36 {
            return 0;
        }
        ffi::av_display_rotation_get((*side).data as *const i32)
    };
    if !degrees.is_finite() {
        return 0;
    }
    // FFmpeg gives the angle counter-clockwise.
    ((-degrees / 90.0).round() as i64).rem_euclid(4) as u8
}

/// What the video in `stream` is, before anything is decoded. `has_sound` is left for the caller to decide.
fn describe(input: &Input, index: usize) -> Result<VideoInfo, MediaError> {
    let stream = input.stream(index).expect("the index came from this input");
    let codec = stream.parameters().id();
    if ffmpeg_next::decoder::find(codec).is_none() {
        return Err(MediaError::Unsupported(format!("Sortier can't decode {} video", codec.name())));
    }
    let parameters = raw_parameters(input, index);
    // SAFETY: `parameters` is valid while `input` is.
    let (mut width, mut height) = unsafe { ((*parameters).width.max(0) as u32, (*parameters).height.max(0) as u32) };
    if width == 0 || height == 0 {
        return Err(MediaError::Decode("the video doesn't say how large it is".into()));
    }
    if turns_of(input, index) % 2 == 1 {
        std::mem::swap(&mut width, &mut height);
    }
    let duration = if input.duration() > 0 {
        input.duration() as f64 / f64::from(ffi::AV_TIME_BASE)
    } else {
        stream.duration().max(0) as f64 * seconds_per_tick(stream.time_base())
    };
    if duration <= 0.0 {
        return Err(MediaError::Decode("the video doesn't say how long it is".into()));
    }
    let average = seconds_per_tick(stream.avg_frame_rate());
    let rate = if average > 0.0 { average } else { seconds_per_tick(stream.rate()) };
    Ok(VideoInfo {
        width,
        height,
        duration: crate::duration(duration),
        frame_rate: plausible_rate(rate),
        has_sound: false,
    })
}

/// Reads a video's size, length, frame rate and whether it has sound, without decoding any of it.
pub(crate) fn probe(path: &Path) -> Result<VideoInfo, MediaError> {
    let input = open_input(path)?;
    let index = video_stream(&input)?;
    let mut info = describe(&input, index)?;
    info.has_sound = sound_stream(&input).is_some();
    Ok(info)
}

/// Where a thumbnail is taken from: about a tenth of the way in, but no later than a second, so it's past any fade
/// from black; the first frame of clips shorter than a second.
fn thumbnail_time(duration: f64) -> f64 {
    if duration < 1.0 { 0.0 } else { (duration * 0.1).min(1.0) }
}

/// One representative frame of a video (see [`thumbnail_time`]), shrunk so its longer side is at most `max_side`.
/// Decodes on the processor, or on the graphics card for codecs only it can decode here. Blocks while decoding.
pub(crate) fn thumbnail(path: &Path, max_side: u32) -> Result<Picture, MediaError> {
    match thumbnail_with(path, max_side, None) {
        Err(error @ MediaError::Decode(_)) => match detect_hardware_decoder() {
            Some(hardware) => thumbnail_with(path, max_side, Some(hardware)),
            None => Err(error),
        },
        result => result,
    }
}

fn thumbnail_with(path: &Path, max_side: u32, hardware: Option<HardwareDecoder>) -> Result<Picture, MediaError> {
    let mut video = VideoDecoder::open(&VideoSource::File(path.to_path_buf()), hardware)?;
    for at in [thumbnail_time(crate::seconds(video.info.duration)), 0.0] {
        video.seek(at)?;
        if let Some((_, picture)) = video.next_sized(at, Some(max_side))? {
            return Ok(picture);
        }
    }
    Err(MediaError::Decode("the video has no pictures".into()))
}

fn open_decoder(input: &Input, index: usize, hardware: Option<HardwareDecoder>) -> Result<ffmpeg_next::decoder::Video, MediaError> {
    let stream = input.stream(index).expect("the index came from this input");
    let mut context = codec::Context::from_parameters(stream.parameters()).map_err(media_error)?;
    context.set_threading(threading::Config {
        kind: threading::Type::Frame,
        count: 0,
    });
    if let Some(device) = hardware.and_then(HardwareDecoder::open_device) {
        // SAFETY: the codec context isn't open yet, so it may be given a device; it keeps its own reference, which
        // it releases when freed. FFmpeg's default format choice then takes the device's format when the codec
        // can be decoded on it, and a processor format when it can't.
        unsafe { (*context.as_mut_ptr()).hw_device_ctx = device.new_reference() };
    }
    context.decoder().video().map_err(media_error)
}

/// A decoded frame and the moment of the video it's shown from.
struct Decoded {
    time: f64,
    frame: frame::Video,
}

/// A video's pictures from wherever it was last sought to: on the graphics card when given a hardware decoder that
/// can take the video's codec, otherwise on the processor.
pub(crate) struct VideoDecoder {
    pub(crate) info: VideoInfo,
    reader: StreamReader,
    turns: u8,
    decoder: ffmpeg_next::decoder::Video,
    /// The hardware decoder asked for; cleared if it fails before giving any picture, so decoding carries on on
    /// the processor.
    hardware: Option<HardwareDecoder>,
    /// The hardware decoder the last picture came from.
    in_use: Option<HardwareDecoder>,
    scaler: Option<Scaler>,
    /// A frame decoded while looking ahead, given next.
    held: Option<Decoded>,
    /// Where the last seek went: frames whose successor is shown by then are skipped.
    from: f64,
    last_time: Option<f64>,
    gave_picture: bool,
}

impl VideoDecoder {
    /// Opens `source` at its start, decoding with `hardware` where it can.
    pub(crate) fn open(source: &VideoSource, hardware: Option<HardwareDecoder>) -> Result<Self, MediaError> {
        let input = open_source(source)?;
        let stream = video_stream(&input)?;
        let info = describe(&input, stream)?;
        let turns = turns_of(&input, stream);
        let decoder = open_decoder(&input, stream, hardware)?;
        Ok(Self {
            info,
            reader: StreamReader::new(input, stream),
            turns,
            decoder,
            hardware,
            in_use: None,
            scaler: None,
            held: None,
            from: 0.0,
            last_time: None,
            gave_picture: false,
        })
    }

    /// The graphics card decoder the last picture came from, or `None` when it was decoded on the processor.
    pub(crate) fn hardware_in_use(&self) -> Option<HardwareDecoder> {
        self.in_use
    }

    /// Goes to `seconds` in; the next picture is the one showing then.
    pub(crate) fn seek(&mut self, seconds: f64) -> Result<(), MediaError> {
        self.reader.seek(seconds)?;
        self.decoder.flush();
        self.held = None;
        self.from = seconds;
        self.last_time = None;
        Ok(())
    }

    /// The next picture worth showing at `position` and its time, skipping frames whose successor is already due;
    /// `None` at the end of the video.
    pub(crate) fn next_frame(&mut self, position: f64) -> Result<Option<(f64, Picture)>, MediaError> {
        self.next_sized(position, None)
    }

    /// [`Self::next_frame`], shrunk so its longer side is at most `max_side` when one is given.
    fn next_sized(&mut self, position: f64, max_side: Option<u32>) -> Result<Option<(f64, Picture)>, MediaError> {
        match self.next_picture(position, max_side) {
            Err(_) if self.hardware.is_some() && !self.gave_picture => {
                self.hardware = None;
                self.decoder = open_decoder(&self.reader.input, self.reader.index, None)?;
                self.seek(self.from)?;
                self.next_picture(position, max_side)
            }
            result => result,
        }
    }

    fn next_picture(&mut self, position: f64, max_side: Option<u32>) -> Result<Option<(f64, Picture)>, MediaError> {
        let due = position.max(self.from);
        let mut current = match self.held.take() {
            Some(held) => held,
            None => match self.decode()? {
                Some(decoded) => decoded,
                None => return Ok(None),
            },
        };
        while let Some(next) = self.decode()? {
            if next.time > due {
                self.held = Some(next);
                break;
            }
            current = next;
        }
        let picture = self.picture(&current.frame, max_side)?;
        self.gave_picture = true;
        Ok(Some((current.time, picture)))
    }

    /// The next frame out of the decoder; `None` once it has given them all.
    fn decode(&mut self) -> Result<Option<Decoded>, MediaError> {
        let mut frame = frame::Video::empty();
        if !self.reader.decode(&mut self.decoder, &mut frame)? {
            return Ok(None);
        }
        Ok(Some(self.timed(frame)))
    }

    fn timed(&mut self, frame: frame::Video) -> Decoded {
        let time = match frame.timestamp().or_else(|| frame.pts()) {
            Some(ticks) => self.reader.seconds(ticks),
            None => self.last_time.map_or(self.from, |last| last + 1.0 / self.info.frame_rate),
        };
        self.last_time = Some(time);
        Decoded {
            time: time.max(0.0),
            frame,
        }
    }

    /// `frame` as an upright RGBA picture, copied from the graphics card first if it's there.
    fn picture(&mut self, frame: &frame::Video, max_side: Option<u32>) -> Result<Picture, MediaError> {
        // SAFETY: the frame was decoded, so its format is set.
        let format = unsafe { (*frame.as_ptr()).format };
        self.in_use = self.hardware.filter(|hardware| format == hardware.pixel_format() as i32);
        let downloaded = match self.in_use {
            Some(_) => Some(download(frame)?),
            None => None,
        };
        let source = downloaded.as_ref().unwrap_or(frame);
        let (width, height) = match max_side {
            Some(side) => fit_size(source.width(), source.height(), side),
            None => (source.width(), source.height()),
        };
        let key = ScalerKey::of(source, width, height);
        if self.scaler.as_ref().is_none_or(|scaler| scaler.key != key) {
            self.scaler = Some(Scaler::new(key)?);
        }
        let rgba = self.scaler.as_mut().expect("made above").run(source)?;
        Ok(turn(Picture { width, height, rgba }, self.turns))
    }
}

/// A frame on the graphics card copied into memory, in the card's own pixel format (usually NV12).
fn download(frame: &frame::Video) -> Result<frame::Video, MediaError> {
    let mut downloaded = frame::Video::empty();
    // SAFETY: both frames are valid; the empty one is filled by FFmpeg, then given the source's colour details.
    let result = unsafe {
        let result = ffi::av_hwframe_transfer_data(downloaded.as_mut_ptr(), frame.as_ptr(), 0);
        if result >= 0 {
            ffi::av_frame_copy_props(downloaded.as_mut_ptr(), frame.as_ptr());
        }
        result
    };
    if result < 0 {
        return Err(MediaError::Decode(format!("a picture couldn't be copied from the graphics card: {}", Error::from(result))));
    }
    Ok(downloaded)
}

/// What a [`Scaler`] converts from and to.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ScalerKey {
    format: Pixel,
    width: u32,
    height: u32,
    out_width: u32,
    out_height: u32,
    space: color::Space,
    full_range: bool,
}

impl ScalerKey {
    fn of(frame: &frame::Video, out_width: u32, out_height: u32) -> Self {
        Self {
            format: frame.format(),
            width: frame.width(),
            height: frame.height(),
            out_width,
            out_height,
            space: frame.color_space(),
            full_range: frame.color_range() == color::Range::JPEG,
        }
    }

    /// swscale's coefficients for the frame's colour space. Videos that don't say are taken to be BT.709 when
    /// they're HD and BT.601 when they're not, as players do.
    fn coefficients(self) -> i32 {
        match self.space {
            color::Space::BT709 => ffi::SWS_CS_ITU709,
            color::Space::BT2020NCL | color::Space::BT2020CL => ffi::SWS_CS_BT2020,
            color::Space::FCC => ffi::SWS_CS_FCC,
            color::Space::SMPTE240M => ffi::SWS_CS_SMPTE240M,
            color::Space::SMPTE170M | color::Space::BT470BG => ffi::SWS_CS_ITU601,
            _ if self.height >= 720 => ffi::SWS_CS_ITU709,
            _ => ffi::SWS_CS_ITU601,
        }
    }
}

/// Converts frames of one format and size to straight RGBA of another size, with the frame's colour space.
struct Scaler {
    context: *mut ffi::SwsContext,
    key: ScalerKey,
}

// SAFETY: a scaling context has no ties to the thread that made it, and is only used through `&mut self`.
unsafe impl Send for Scaler {}

impl Scaler {
    fn new(key: ScalerKey) -> Result<Self, MediaError> {
        let shrinking = key.out_width < key.width || key.out_height < key.height;
        let flags = if shrinking { scaling::Flags::AREA } else { scaling::Flags::BILINEAR };
        // SAFETY: the sizes and formats are plain values; a null context is checked before use.
        let context = unsafe {
            ffi::sws_getContext(
                key.width as i32,
                key.height as i32,
                key.format.into(),
                key.out_width as i32,
                key.out_height as i32,
                ffi::AVPixelFormat::AV_PIX_FMT_RGBA,
                flags.bits(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if context.is_null() {
            return Err(MediaError::Unsupported(format!("Sortier can't convert {:?} pictures", key.format)));
        }
        // SAFETY: the context was just made; the coefficient tables are FFmpeg's own statics.
        unsafe {
            ffi::sws_setColorspaceDetails(
                context,
                ffi::sws_getCoefficients(key.coefficients()),
                i32::from(key.full_range),
                ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT),
                1,
                0,
                1 << 16,
                1 << 16,
            );
        }
        Ok(Self { context, key })
    }

    /// `frame`, which must match the key, as tightly packed RGBA rows.
    fn run(&mut self, frame: &frame::Video) -> Result<Vec<u8>, MediaError> {
        let (width, height) = (self.key.out_width as usize, self.key.out_height as usize);
        let mut rgba = vec![0_u8; width * height * 4];
        let destination = [rgba.as_mut_ptr(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()];
        let strides = [(width * 4) as i32, 0, 0, 0];
        // SAFETY: the frame matches the context's input, and `rgba` holds exactly the output's rows.
        let rows = unsafe {
            let source = frame.as_ptr();
            ffi::sws_scale(
                self.context,
                (*source).data.as_ptr() as *const *const u8,
                (*source).linesize.as_ptr(),
                0,
                (*source).height,
                destination.as_ptr(),
                strides.as_ptr(),
            )
        };
        if rows <= 0 {
            return Err(MediaError::Decode("a picture couldn't be converted".into()));
        }
        Ok(rgba)
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: the context is this scaler's own, and nothing uses it after this.
        unsafe { ffi::sws_freeContext(self.context) };
    }
}

/// `picture` turned `turns` quarter turns clockwise.
fn turn(picture: Picture, turns: u8) -> Picture {
    let turns = turns % 4;
    if turns == 0 {
        return picture;
    }
    let (width, height) = (picture.width as usize, picture.height as usize);
    let (out_width, out_height) = if turns == 2 { (width, height) } else { (height, width) };
    let mut rgba = vec![0; picture.rgba.len()];
    for y in 0..height {
        for x in 0..width {
            let (to_x, to_y) = match turns {
                1 => (height - 1 - y, x),
                2 => (width - 1 - x, height - 1 - y),
                _ => (y, width - 1 - x),
            };
            let from = (y * width + x) * 4;
            let to = (to_y * out_width + to_x) * 4;
            rgba[to..to + 4].copy_from_slice(&picture.rgba[from..from + 4]);
        }
    }
    Picture {
        width: out_width as u32,
        height: out_height as u32,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_turn_clockwise() {
        // Two pixels side by side: red, then green.
        let picture = Picture {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        let quarter = turn(picture.clone(), 1);
        assert_eq!((quarter.width, quarter.height), (1, 2));
        assert_eq!(&quarter.rgba[..4], &[255, 0, 0, 255], "red ends up on top");
        let half = turn(picture.clone(), 2);
        assert_eq!(&half.rgba[..4], &[0, 255, 0, 255], "green ends up on the left");
        let three = turn(picture, 3);
        assert_eq!(&three.rgba[..4], &[0, 255, 0, 255], "green ends up on top");
    }

    #[test]
    fn thumbnails_come_from_just_past_the_start() {
        assert_eq!(thumbnail_time(0.5), 0.0);
        assert_eq!(thumbnail_time(5.0), 0.5);
        assert_eq!(thumbnail_time(60.0), 1.0);
    }

    #[test]
    fn implausible_frame_rates_are_replaced() {
        assert_eq!(plausible_rate(29.97), 29.97);
        assert_eq!(plausible_rate(90_000.0), 30.0);
        assert_eq!(plausible_rate(0.0), 30.0);
    }
}
