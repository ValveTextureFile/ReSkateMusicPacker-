// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! ffmpeg and ffprobe, and the folders the packer works in.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::binary::Sha1;
use crate::error::IoContext;
use crate::{Error, Result};

/// A folder the user pointed at that holds ffmpeg and ffprobe; tried before PATH.
static TOOL_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// "ffmpeg" -> "ffmpeg.exe" on Windows.
pub fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn on_path(file: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(file)).find(|candidate| candidate.is_file())
}

/// The program to run for `name`: the configured folder's copy, else whatever PATH finds.
pub fn program(name: &str) -> PathBuf {
    let file = executable(name);
    if let Some(dir) = TOOL_DIR.read().expect("tool dir lock").as_ref() {
        let candidate = dir.join(&file);
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(file)
}

/// Uses `extra` (when it holds ffmpeg; its `bin` subfolder also counts) and reports whether both
/// ffmpeg and ffprobe can now be found. Returns the folder that was taken, if any.
pub fn locate(extra: Option<&Path>) -> (bool, Option<PathBuf>) {
    let mut taken = None;
    if let Some(extra) = extra.filter(|dir| !dir.as_os_str().is_empty()) {
        let folder =
            if !extra.join(executable("ffmpeg")).is_file() && extra.join("bin").join(executable("ffmpeg")).is_file() {
                extra.join("bin")
            } else {
                extra.to_path_buf()
            };
        if folder.join(executable("ffmpeg")).is_file() {
            *TOOL_DIR.write().expect("tool dir lock") = Some(folder.clone());
            taken = Some(folder);
        }
    }
    let found = |name: &str| {
        let program = program(name);
        program.is_file() || on_path(&executable(name)).is_some()
    };
    (found("ffmpeg") && found("ffprobe"), taken)
}

/// A command for ffmpeg or ffprobe that never flashes a console window.
pub fn command(name: &str) -> Command {
    let mut command = Command::new(program(name));
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn describe(command: &Command) -> String {
    let quote = |part: &OsStr| {
        let text = part.to_string_lossy();
        if text.contains(' ') { format!("\"{text}\"") } else { text.into_owned() }
    };
    std::iter::once(quote(command.get_program())).chain(command.get_args().map(quote)).collect::<Vec<_>>().join(" ")
}

fn spawn_failure(command: &Command, error: std::io::Error) -> Error {
    let name = command.get_program().to_string_lossy().into_owned();
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::msg(format!("{name} was not found; install ffmpeg or point the packer at it"))
    } else {
        Error::io(format!("Cannot run {name}"), error)
    }
}

/// Runs to completion; a failure carries ffmpeg's last few lines of error output.
pub fn run(mut command: Command) -> Result<()> {
    let output =
        command.stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| spawn_failure(&command, e))?;
    if output.status.success() {
        return Ok(());
    }
    let errors = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = errors.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = tail[tail.len().saturating_sub(3)..].join(" / ");
    if tail.is_empty() {
        Err(Error::msg(format!("Command failed: {}", describe(&command))))
    } else {
        Err(Error::msg(format!("Command failed: {} ({tail})", describe(&command))))
    }
}

/// Runs to completion and returns stdout followed by stderr.
pub fn run_capture(mut command: Command) -> Result<String> {
    let output =
        command.stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| spawn_failure(&command, e))?;
    if !output.status.success() {
        return Err(Error::msg(format!("Command failed: {}", describe(&command))));
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

/// Where scan and pack keep ffmpeg's intermediate output.
pub fn scratch_folder() -> Result<PathBuf> {
    let folder = std::env::temp_dir().join("ReSkateMusicPacker");
    std::fs::create_dir_all(&folder).at("Cannot create", &folder)?;
    Ok(folder)
}

/// A fresh file name in the scratch folder, unique within this process.
pub fn scratch_file(stem: &str, extension: &str) -> Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(scratch_folder()?.join(format!("{stem}-{}-{n}.{extension}", std::process::id())))
}

/// Where encoded songs and extracted covers are cached between builds.
pub fn cache_folder() -> PathBuf {
    let folder = match dirs::cache_dir() {
        Some(base) => base.join("ReSkateMusicPacker").join("cache"),
        None => std::env::temp_dir().join("ReSkateMusicPacker").join("cache"),
    };
    let _ = std::fs::create_dir_all(&folder);
    folder
}

pub fn sha1_of_file(path: &Path) -> Result<Sha1> {
    use sha1::Digest;
    let mut file = File::open(path).at("Cannot read", path)?;
    let mut hasher = sha1::Sha1::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).at("Cannot read", path)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Sha1(hasher.finalize().into()))
}

pub fn sha256_hex_of_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    let mut file = File::open(path).at("Cannot read", path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).at("Cannot read", path)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Writes a file, creating its folder first.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).at("Cannot create", parent)?;
    }
    std::fs::write(path, bytes).at("Cannot write", path)
}
