// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
// One program for both faces: arguments run the command line, none (or `--gui`) open the window.
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

mod cli;
#[cfg(feature = "gui")]
mod gui;

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().collect();
    // No arguments, --gui with any songs or folders to add, or files dropped onto the program's
    // icon (bare paths: one of anything, or any audio files; the CLI form is two folders).
    #[cfg(feature = "gui")]
    let dropped = {
        let paths: Vec<&std::path::Path> = arguments.iter().skip(1).map(std::path::Path::new).collect();
        !paths.is_empty()
            && paths.iter().all(|path| path.exists() && !path.to_string_lossy().starts_with('-'))
            && (paths.len() == 1 || paths.iter().any(|path| reskate_music::music::scan::is_audio(path)))
    };
    #[cfg(feature = "gui")]
    if arguments.len() <= 1 || dropped || arguments.iter().skip(1).any(|argument| argument == "--gui") {
        let paths =
            arguments.iter().skip(1).filter(|argument| *argument != "--gui").map(std::path::PathBuf::from).collect();
        return gui::run(paths);
    }
    #[cfg(all(windows, feature = "gui"))]
    console::attach();
    cli::run(arguments)
}

/// A windowed program has no console; the command line borrows the one it was started from.
#[cfg(all(windows, feature = "gui"))]
mod console {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    pub fn attach() {
        // SAFETY: plain Win32 calls; handles that are already set (redirected output) are kept.
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
            for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                let current = GetStdHandle(which);
                if !current.is_null() && current != INVALID_HANDLE_VALUE {
                    continue;
                }
                let handle = CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if handle != INVALID_HANDLE_VALUE {
                    SetStdHandle(which, handle);
                }
            }
        }
    }
}
