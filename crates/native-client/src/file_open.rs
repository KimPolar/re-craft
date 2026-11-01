//! Open a user-selected screenshot without blocking the game event loop.

use anyhow::Result;
use std::path::Path;

#[cfg(windows)]
pub fn open(path: &Path) -> Result<()> {
    use anyhow::{Context, ensure};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        },
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().collect();
    ensure!(
        !path.contains(&0),
        "screenshot path contains a null character"
    );
    let path: Vec<u16> = path.into_iter().chain(std::iter::once(0)).collect();
    std::thread::Builder::new()
        .name("open-screenshot".into())
        .spawn(move || {
            // Shell handlers can block. Initialize COM and open on this worker thread.
            let initialized = unsafe {
                CoInitializeEx(
                    std::ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                )
            };
            if initialized < 0 {
                eprintln!("Cannot initialize the screenshot viewer: {initialized:#x}");
                return;
            }
            // Pass the path directly to the shell API, without invoking a command shell.
            let result = unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    path.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                )
            } as isize;
            if result <= 32 {
                eprintln!("Windows could not open screenshot (error {result})");
            }
            unsafe {
                CoUninitialize();
            }
        })
        .context("cannot start screenshot viewer task")?;
    Ok(())
}

#[cfg(not(windows))]
pub fn open(_: &Path) -> Result<()> {
    anyhow::bail!("opening screenshots is supported on Windows")
}
