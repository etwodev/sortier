//! The bits of Windows the tray app talks to directly: message boxes, the message loop, staying a single instance,
//! and the local time for the log.

use std::path::Path;

use anyhow::{Result, bail};
use sha2::{Digest as _, Sha256};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SYSTEMTIME};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MSG, MessageBoxW,
    PostQuitMessage, PostThreadMessageW, TranslateMessage, WM_APP,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub enum Kind {
    Info,
    Error,
}

/// Shows `text` in a message box of its own and waits for it to be closed.
pub fn message_box(text: &str, kind: Kind) {
    let icon = match kind {
        Kind::Info => MB_ICONINFORMATION,
        Kind::Error => MB_ICONERROR,
    };
    let (text, caption) = (wide(text), wide("Sortier Remote"));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | icon | MB_SETFOREGROUND);
    }
}

pub fn current_thread() -> u32 {
    unsafe { GetCurrentThreadId() }
}

/// Wakes the message loop on `thread`, so it handles what other threads have sent it.
pub fn wake(thread: u32) {
    unsafe {
        PostThreadMessageW(thread, WM_APP, 0, 0);
    }
}

/// Pumps this thread's messages (the tray icon and its menu need it), calling `after_each` after every one, until
/// [`quit_message_loop`].
pub fn run_message_loop(mut after_each: impl FnMut()) {
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
            after_each();
        }
    }
}

pub fn quit_message_loop() {
    unsafe { PostQuitMessage(0) }
}

/// Held while this is the only tray app serving a data folder.
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    /// Claims `data` for this process, or `None` when another tray app already serves it.
    pub fn claim(data: &Path) -> Result<Option<Self>> {
        let key = Sha256::digest(data.to_string_lossy().to_lowercase().as_bytes());
        let name = wide(&format!("Local\\SortierRemoteTray-{}", sortier_remote::setup::hex(&key[..8])));
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            bail!("Sortier Remote can't check whether it's already running: {}", std::io::Error::last_os_error());
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// The local date and time, as `2026-11-16 14:03:05`.
pub fn local_time() -> String {
    let mut time: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut time) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}
