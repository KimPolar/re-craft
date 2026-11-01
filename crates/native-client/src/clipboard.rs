//! Clipboard access is performed only for an explicit chat shortcut.

use anyhow::Result;
use winit::window::Window;

pub trait Clipboard {
    fn read(&mut self) -> Result<String>;
    fn write(&mut self, text: &str) -> Result<()>;
}

#[cfg(any(windows, test))]
fn encode_unicode_text(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(any(windows, test))]
fn decode_unicode_text(data: &[u16]) -> String {
    let end = data
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(data.len());
    String::from_utf16_lossy(&data[..end])
}

pub struct SystemClipboard<'a> {
    window: Option<&'a Window>,
}

impl<'a> SystemClipboard<'a> {
    pub fn new(window: Option<&'a Window>) -> Self {
        Self { window }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use anyhow::{Context, bail, ensure};
    use windows_sys::Win32::{
        Foundation::{GlobalFree, HGLOBAL, HWND},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    struct Open;
    impl Open {
        fn new(window: Option<&Window>) -> Result<Self> {
            let owner: HWND = match window
                .context("clipboard window unavailable")?
                .window_handle()?
                .as_raw()
            {
                RawWindowHandle::Win32(handle) => handle.hwnd.get() as HWND,
                _ => bail!("unsupported clipboard window"),
            };
            // The window belongs to this event-loop thread. No clipboard content is changed here.
            ensure!(
                unsafe { OpenClipboard(owner) } != 0,
                "cannot open clipboard"
            );
            Ok(Self)
        }
    }
    impl Drop for Open {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    struct Allocation(HGLOBAL);
    impl Drop for Allocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    GlobalFree(self.0);
                }
            }
        }
    }

    impl Clipboard for SystemClipboard<'_> {
        fn read(&mut self) -> Result<String> {
            let _open = Open::new(self.window)?;
            let handle = unsafe { GetClipboardData(u32::from(CF_UNICODETEXT)) };
            if handle.is_null() {
                return Ok(String::new());
            }
            // The returned handle is owned by the clipboard and stays valid while it is open.
            let units = (unsafe { GlobalSize(handle) } / 2).min(1_048_576);
            if units == 0 {
                return Ok(String::new());
            }
            let pointer = unsafe { GlobalLock(handle) } as *const u16;
            ensure!(!pointer.is_null(), "cannot read clipboard text");
            let data = unsafe { std::slice::from_raw_parts(pointer, units) };
            let text = decode_unicode_text(data);
            unsafe {
                GlobalUnlock(handle);
            }
            Ok(text)
        }

        fn write(&mut self, text: &str) -> Result<()> {
            let encoded = encode_unicode_text(text);
            let bytes = encoded
                .len()
                .checked_mul(2)
                .context("clipboard text too large")?;
            let mut allocation = Allocation(unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) });
            ensure!(!allocation.0.is_null(), "cannot allocate clipboard text");
            let pointer = unsafe { GlobalLock(allocation.0) } as *mut u16;
            ensure!(!pointer.is_null(), "cannot access clipboard memory");
            unsafe {
                std::ptr::copy_nonoverlapping(encoded.as_ptr(), pointer, encoded.len());
                GlobalUnlock(allocation.0);
            }
            let _open = Open::new(self.window)?;
            ensure!(unsafe { EmptyClipboard() } != 0, "cannot clear clipboard");
            ensure!(
                !unsafe { SetClipboardData(u32::from(CF_UNICODETEXT), allocation.0) }.is_null(),
                "cannot write clipboard text"
            );
            // Successful SetClipboardData transfers ownership to Windows.
            allocation.0 = std::ptr::null_mut();
            Ok(())
        }
    }
}

#[cfg(not(windows))]
impl Clipboard for SystemClipboard<'_> {
    fn read(&mut self) -> Result<String> {
        anyhow::bail!("clipboard is supported on Windows")
    }
    fn write(&mut self, _: &str) -> Result<()> {
        anyhow::bail!("clipboard is supported on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_clipboard_encoding_handles_korean_surrogates_and_terminators() {
        let encoded = encode_unicode_text("한글 😀");
        assert_eq!(encoded.last(), Some(&0));
        assert_eq!(decode_unicode_text(&encoded), "한글 😀");
        assert_eq!(decode_unicode_text(&[0xd800, 0, 0x41]), "�");
        assert_eq!(decode_unicode_text(&[]), "");
    }
}
