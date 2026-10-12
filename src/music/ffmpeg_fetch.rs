// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Fetches and installs ffmpeg + ffprobe when they are missing. The pinned archive is verified
//! with SHA-256 before anything is extracted. Nothing here runs on its own; the window and the CLI
//! call it after an explicit request.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::tools::{executable, sha256_hex_of_file};
use crate::error::IoContext;
use crate::{Error, Result, bail};

/// The pinned asset: BtbN/FFmpeg-Builds autobuild-2026-09-30-13-08, win64 LGPL (static; ffmpeg,
/// ffprobe and libopus). The dated month-end build is retained far longer than the daily ones.
const DEFAULT_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-30-13-08/\
                           ffmpeg-n8.1.3-9-g29e619e767-win64-lgpl-8.1.zip";
const DEFAULT_SHA256: &str = "4a7642b2264c03e8a0ce8a3825b933ee5580656f45695a086fe7e294045ffc0a";
const MAX_DOWNLOAD: u64 = 400 * 1024 * 1024;

/// How far a download has come. `total` is 0 when the server did not report a length.
#[derive(Clone, Copy, Debug, Default)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: u64,
}

/// The build to fetch: `FFMPEG_URL` and `FFMPEG_SHA256` (environment variables) override the
/// pinned one, so link rot or a newer build can be worked around without a rebuild. There is no
/// pinned build for macOS or Linux, where ffmpeg comes from the package manager.
pub fn source() -> Option<(String, String)> {
    let url = std::env::var("FFMPEG_URL").ok().filter(|url| !url.is_empty());
    let sha = std::env::var("FFMPEG_SHA256").ok().filter(|sha| !sha.is_empty());
    match (url, sha) {
        (Some(url), Some(sha)) => Some((url, sha)),
        (url, sha) if cfg!(windows) => {
            Some((url.unwrap_or_else(|| DEFAULT_URL.into()), sha.unwrap_or_else(|| DEFAULT_SHA256.into())))
        }
        _ => None,
    }
}

fn writable_directory(directory: &Path) -> bool {
    if std::fs::create_dir_all(directory).is_err() {
        return false;
    }
    let probe = directory.join(".reskate-write-test");
    let ok = std::fs::write(&probe, [0]).is_ok();
    ok && std::fs::remove_file(&probe).is_ok()
}

/// Where a downloaded ffmpeg lands: `ffmpeg` beside the running program when that folder is
/// writable (portable install), otherwise the user's local data folder. Creates the folder.
pub fn default_install_dir() -> Result<PathBuf> {
    if let Some(portable) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("ffmpeg")))
        && writable_directory(&portable)
    {
        return Ok(portable);
    }
    let fallback = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("ReSkateMusicPacker").join("ffmpeg");
    std::fs::create_dir_all(&fallback).at("Cannot create", &fallback)?;
    Ok(fallback)
}

/// True when `file`'s SHA-256 equals `expected` (case-insensitive).
pub fn verify_sha256(file: &Path, expected: &str) -> Result<bool> {
    Ok(sha256_hex_of_file(file)?.eq_ignore_ascii_case(expected.trim()))
}

fn copy_with_progress(
    reader: &mut dyn Read,
    dest: &Path,
    total: u64,
    progress: &mut dyn FnMut(DownloadProgress),
    cancel: &AtomicBool,
) -> Result<()> {
    let mut out = File::create(dest).at("Cannot write", dest)?;
    let mut buffer = vec![0u8; 1 << 16];
    let mut received = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let read = reader.read(&mut buffer).map_err(|_| Error::msg("The ffmpeg download was interrupted."))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read]).at("Cannot write", dest)?;
        received += read as u64;
        if received > MAX_DOWNLOAD {
            bail!("The ffmpeg download is larger than expected.");
        }
        progress(DownloadProgress { received, total });
    }
    if total != 0 && received != total {
        bail!("The ffmpeg download stopped before it was complete.");
    }
    Ok(())
}

/// Copies `url` to `dest` (an existing file is replaced). `http(s)://` is downloaded, following
/// redirects, with timeouts and a size cap; anything else is a local path (or `file://...`), which
/// keeps the tests offline and lets FFMPEG_URL point at a mirror.
pub fn download_to_file(
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(DownloadProgress),
    cancel: &AtomicBool,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .max_redirects(10)
            .http_status_as_error(false)
            .user_agent("ReSkateMusicPacker/2");
        #[cfg(windows)]
        let config =
            config.tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build());
        let config = config.build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent
            .get(url)
            .call()
            .map_err(|_| Error::msg("The ffmpeg download could not start (offline, blocked or the link moved)."))?;
        let status = response.status().as_u16();
        if status != 200 {
            bail!("The ffmpeg download failed (HTTP {status}).");
        }
        let total = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        if total > MAX_DOWNLOAD {
            bail!("The ffmpeg download is larger than expected.");
        }
        let mut reader = response.into_body().into_reader();
        return copy_with_progress(&mut reader, dest, total, progress, cancel);
    }
    let path = PathBuf::from(url.strip_prefix("file://").unwrap_or(url));
    let total = std::fs::metadata(&path).map_err(|_| Error::msg(format!("Cannot read {}", path.display())))?.len();
    let mut file = File::open(&path).at("Cannot read", &path)?;
    copy_with_progress(&mut file, dest, total, progress, cancel)
}

/// Extracts one zip member into `dest`. `member` is a forward-slash path, matched exactly or as
/// the suffix of an entry (so "bin/ffmpeg.exe" finds "ffmpeg-.../bin/ffmpeg.exe").
pub fn extract_zip_member(archive: &Path, member: &str, dest: &Path) -> Result<()> {
    let file = File::open(archive).at("Cannot read", archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| Error::msg("The downloaded ffmpeg archive is not a zip."))?;
    let suffix = format!("/{member}");
    let mut found = None;
    for index in 0..zip.len() {
        let Ok(entry) = zip.by_index(index) else { continue };
        // A few zip writers still store Windows-style paths.
        let name = entry.name().map(|name| name.replace('\\', "/")).unwrap_or_default();
        if name == member || name.ends_with(&suffix) {
            found = Some(index);
            break;
        }
    }
    let Some(index) = found else { bail!("The ffmpeg archive has no {member}.") };
    let mut entry =
        zip.by_index(index).map_err(|_| Error::msg(format!("Cannot read {member} from the ffmpeg archive.")))?;
    if entry.size() == 0 || entry.size() > MAX_DOWNLOAD {
        bail!("The ffmpeg archive's {member} has an unexpected size.");
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).at("Cannot create", parent)?;
    }
    let mut out = File::create(dest).at("Cannot write", dest)?;
    std::io::copy(&mut entry, &mut out)
        .map_err(|_| Error::msg(format!("Cannot extract {member} from the ffmpeg archive.")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// Downloads `url`, verifies its SHA-256 and extracts ffmpeg + ffprobe into `install_dir`,
/// returning that folder. The URL and hash are parameters so there is no hidden network use and
/// tests can pass a local fixture.
pub fn ensure_ffmpeg(
    install_dir: &Path,
    url: &str,
    sha256: &str,
    progress: &mut dyn FnMut(DownloadProgress),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    std::fs::create_dir_all(install_dir).at("Cannot create", install_dir)?;
    let archive = install_dir.join("ffmpeg-download.zip");
    let result = (|| {
        download_to_file(url, &archive, progress, cancel)?;
        if !verify_sha256(&archive, sha256)? {
            bail!("The ffmpeg download did not match its expected checksum and was rejected.");
        }
        for name in ["ffmpeg", "ffprobe"] {
            let file = executable(name);
            extract_zip_member(&archive, &format!("bin/{file}"), &install_dir.join(&file))?;
        }
        Ok(install_dir.to_path_buf())
    })();
    let _ = std::fs::remove_file(&archive);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn fixture_zip(path: &Path, top: &str) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, content) in [
            (format!("{top}/bin/{}", executable("ffmpeg")), "this is the fake ffmpeg binary"),
            (format!("{top}/bin/{}", executable("ffprobe")), "this is the fake ffprobe binary"),
            (format!("{top}/LICENSE.txt"), "LGPL notice"),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reskate-fetch-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn installs_from_a_verified_local_archive() {
        let dir = scratch("ok");
        let archive = dir.join("fixture.zip");
        fixture_zip(&archive, "ffmpeg-n8-win64");
        let sha = sha256_hex_of_file(&archive).unwrap();
        assert!(verify_sha256(&archive, &sha.to_uppercase()).unwrap());
        let install = dir.join("install");
        let mut steps = 0;
        let cancel = AtomicBool::new(false);
        let url = format!("file://{}", archive.display());
        ensure_ffmpeg(&install, &url, &sha, &mut |_| steps += 1, &cancel).unwrap();
        assert!(steps > 0);
        let installed = std::fs::read_to_string(install.join(executable("ffmpeg"))).unwrap();
        assert_eq!(installed, "this is the fake ffmpeg binary");
        assert!(install.join(executable("ffprobe")).is_file());
        assert!(!install.join("ffmpeg-download.zip").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_checksum_mismatch_is_rejected_and_nothing_is_installed() {
        let dir = scratch("bad");
        let archive = dir.join("fixture.zip");
        fixture_zip(&archive, "top");
        let install = dir.join("install");
        let cancel = AtomicBool::new(false);
        let error =
            ensure_ffmpeg(&install, archive.to_str().unwrap(), &"0".repeat(64), &mut |_| {}, &cancel).unwrap_err();
        assert!(error.to_string().contains("checksum"));
        assert!(!install.join(executable("ffmpeg")).exists());
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            download_to_file(archive.to_str().unwrap(), &dir.join("x"), &mut |_| {}, &cancelled),
            Err(Error::Cancelled)
        ));
        assert!(extract_zip_member(&archive, "bin/missing.exe", &dir.join("y")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
