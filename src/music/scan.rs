// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! What the songs are called and how long they are, read with ffprobe.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::SongInfo;
use super::text::{trim_tag, usable_name};
use super::tools::{self, run_capture};

/// The stream index of the first attached picture (cover art), if any.
pub fn attached_picture(root: &Value) -> Option<u64> {
    root.get("streams")?.as_array()?.iter().find_map(|stream| {
        let attached = stream.get("disposition")?.get("attached_pic")?.as_i64()? == 1;
        if attached { stream.get("index")?.as_u64() } else { None }
    })
}

/// The longest song the codec header can describe: 26 bits of 48 kHz samples (23 minutes).
pub const MAX_SAMPLES: f64 = (1u64 << 26) as f64;

fn probe(file: &Path) -> crate::Result<Value> {
    let mut command = tools::command("ffprobe");
    command
        .args(["-v", "error", "-show_entries"])
        .arg("format=duration:format_tags=artist,title:stream=index:stream_disposition=attached_pic")
        .args(["-of", "json"])
        .arg(file);
    let text = run_capture(command)?;
    serde_json::from_str(&text).map_err(|error| crate::Error::msg(format!("ffprobe output: {error}")))
}

/// Tags and length of one file, without encoding. Never fails: a bad file gets a problem entry.
pub fn scan_one(file: &Path) -> SongInfo {
    let mut song = SongInfo { file: file.to_path_buf(), ..Default::default() };
    match probe(file) {
        Ok(root) => {
            song.has_embedded_artwork = attached_picture(&root).is_some();
            let format = root.get("format");
            if let Some(duration) = format.and_then(|format| format.get("duration")).and_then(Value::as_str) {
                song.seconds = duration.trim().parse().unwrap_or(0.0);
            }
            if let Some(tags) = format.and_then(|format| format.get("tags")) {
                let tag = |keys: [&str; 3]| keys.iter().find_map(|key| tags.get(*key)?.as_str().map(trim_tag));
                song.artist = tag(["artist", "ARTIST", "Artist"]).unwrap_or_default();
                song.title = tag(["title", "TITLE", "Title"]).unwrap_or_default();
            }
        }
        Err(_) => song.problems.push("ffprobe could not read it".into()),
    }
    if song.artist.is_empty() {
        song.problems.push("no artist tag".into());
    }
    if song.title.is_empty() {
        song.problems.push("no title tag".into());
    }
    if song.artist.is_empty() || song.title.is_empty() {
        // "Artist - Title.mp3" names a song as well as its tags do.
        let stem = file.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let split = stem.split_once(" - ");
        if song.artist.is_empty() {
            song.artist = split.map_or_else(|| "Unknown Artist".into(), |(artist, _)| trim_tag(artist));
        }
        if song.title.is_empty() {
            song.title = trim_tag(split.map_or(stem.as_str(), |(_, title)| title));
        }
    }
    if !usable_name(&song.artist) || !usable_name(&song.title) {
        song.problems.push("artist or title is empty, too long, or has control characters".into());
    }
    if song.seconds * 48000.0 >= MAX_SAMPLES {
        song.problems.push("too long (the codec header stores 26 bits of samples: 23 minutes at most)".into());
    }
    song
}

/// Tags and length of each file. Never fails for a bad file: that is a problem entry.
pub fn scan(files: &[PathBuf]) -> Vec<SongInfo> {
    files.iter().map(|file| scan_one(file)).collect()
}

/// The audio file extensions a dropped or chosen folder brings in.
pub const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "ogg", "opus", "wav", "m4a", "aac", "wma", "aiff", "aif", "webm", "mka", "mp4"];

/// Image extensions the CLI skips when it reads a song folder.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "avif", "tif", "tiff", "ico"];

fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
        let extension = extension.to_ascii_lowercase();
        list.contains(&extension.as_str())
    })
}

pub fn is_audio(path: &Path) -> bool {
    has_extension(path, AUDIO_EXTENSIONS)
}

pub fn is_image(path: &Path) -> bool {
    has_extension(path, IMAGE_EXTENSIONS)
}

/// Dropped or chosen paths as audio files: a folder brings in its audio files, subfolders
/// included. The result is sorted by path, with duplicates removed.
pub fn expand(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(directory: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&path, out),
                Ok(kind) if kind.is_file() && is_audio(&path) => out.push(path),
                _ => {}
            }
        }
    }
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, &mut files);
        } else {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_pictures_are_found_by_disposition() {
        let root: Value = serde_json::from_str(
            r#"{"streams":[{"index":0,"disposition":{"attached_pic":0}},{"index":1,"disposition":{"attached_pic":1}}]}"#,
        )
        .unwrap();
        assert_eq!(attached_picture(&root), Some(1));
        assert_eq!(attached_picture(&serde_json::json!({})), None);
    }

    #[test]
    fn a_missing_file_is_a_problem_not_a_failure() {
        let song = scan_one(Path::new("/definitely/not/here/Artist - Title.mp3"));
        assert!(!song.problems.is_empty());
        assert_eq!(song.artist, "Artist");
        assert_eq!(song.title, "Title");
    }
}
