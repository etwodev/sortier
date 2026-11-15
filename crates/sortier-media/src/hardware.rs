use std::ptr;

use ffmpeg_next::ffi;

/// A graphics card video decoder FFmpeg can use on this system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HardwareDecoder {
    /// Apple's decoder on macOS.
    VideoToolbox,
    /// Direct3D 11 video acceleration: any recent GPU on Windows (NVIDIA, AMD or Intel).
    D3d11va,
    /// DirectX Video Acceleration 2, for older Windows drivers.
    Dxva2,
    /// VA-API on Linux: Intel and AMD.
    Vaapi,
}

impl HardwareDecoder {
    /// What to call it in the interface.
    pub fn label(self) -> &'static str {
        match self {
            HardwareDecoder::VideoToolbox => "VideoToolbox",
            HardwareDecoder::D3d11va => "Direct3D 11",
            HardwareDecoder::Dxva2 => "DXVA 2",
            HardwareDecoder::Vaapi => "VA-API",
        }
    }

    /// The decoders worth trying on this system, best first.
    fn candidates() -> &'static [HardwareDecoder] {
        if cfg!(target_os = "macos") {
            &[HardwareDecoder::VideoToolbox]
        } else if cfg!(target_os = "windows") {
            &[HardwareDecoder::D3d11va, HardwareDecoder::Dxva2]
        } else {
            &[HardwareDecoder::Vaapi]
        }
    }

    fn device_type(self) -> ffi::AVHWDeviceType {
        match self {
            HardwareDecoder::VideoToolbox => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
            HardwareDecoder::D3d11va => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            HardwareDecoder::Dxva2 => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2,
            HardwareDecoder::Vaapi => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
        }
    }

    /// The pixel format of the frames this decoder leaves on the graphics card.
    pub(crate) fn pixel_format(self) -> ffi::AVPixelFormat {
        match self {
            HardwareDecoder::VideoToolbox => ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX,
            HardwareDecoder::D3d11va => ffi::AVPixelFormat::AV_PIX_FMT_D3D11,
            HardwareDecoder::Dxva2 => ffi::AVPixelFormat::AV_PIX_FMT_DXVA2_VLD,
            HardwareDecoder::Vaapi => ffi::AVPixelFormat::AV_PIX_FMT_VAAPI,
        }
    }

    /// Opens the decoder's device, or `None` when this build of FFmpeg doesn't include it or the computer has no
    /// graphics card it can use.
    pub(crate) fn open_device(self) -> Option<Device> {
        let mut device = ptr::null_mut();
        // SAFETY: `device` is a valid out-pointer; the null device name and options ask for the default device.
        let result = unsafe { ffi::av_hwdevice_ctx_create(&mut device, self.device_type(), ptr::null(), ptr::null_mut(), 0) };
        (result >= 0 && !device.is_null()).then_some(Device(device))
    }
}

/// An open hardware decoding device: a reference FFmpeg counts, released when dropped.
pub(crate) struct Device(*mut ffi::AVBufferRef);

// SAFETY: FFmpeg's buffer references are reference counted atomically and may be released from any thread.
unsafe impl Send for Device {}

impl Device {
    /// A new reference to the device, for a codec context to own.
    pub(crate) fn new_reference(&self) -> *mut ffi::AVBufferRef {
        // SAFETY: `self.0` is a live buffer reference for as long as `self` is.
        unsafe { ffi::av_buffer_ref(self.0) }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the reference this `Device` owns, and nothing uses it after this.
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// The best graphics card decoder this computer has, or `None` when video can only be decoded on the processor.
/// Opening a device can take a moment on some systems, so call it off the UI thread.
///
/// Opening is checked, not every codec: a card that can't decode a particular video (AV1 on an older card, say)
/// is found out when it's played, and that video is decoded on the processor instead.
pub fn detect_hardware_decoder() -> Option<HardwareDecoder> {
    HardwareDecoder::candidates().iter().copied().find(|decoder| decoder.open_device().is_some())
}
