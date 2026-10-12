// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The packer: songs in, an add-only music mod out. Needs ffmpeg and ffprobe.
//!
//! [`scan`] reads what the songs are called, [`pack`] builds the mod, [`load_project`] reopens
//! one and [`export_thunderstore`] packages it for upload. One scan or pack at a time per process
//! is the intended use; they share a scratch folder (each run's files are uniquely named).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub mod artwork;
pub mod audio;
pub mod ffmpeg_fetch;
mod pack;
mod project;
pub mod scan;
pub mod text;
mod thunderstore;
pub mod tools;
pub mod wave;

pub use artwork::{embedded_artwork, image_artwork_png, playlist_artwork_png};
pub use pack::pack;
pub use project::{Project, load_project};
pub use scan::{expand, scan, scan_one};
pub use text::{sanitize_text, slug_of, unique_slug, usable_name};
pub use thunderstore::{ThunderstoreOptions, export_thunderstore, thunderstore_name};

/// A song as the packer sees it.
#[derive(Clone, Debug, Default)]
pub struct SongInfo {
    pub file: PathBuf,
    /// From the tags, else the "Artist - Title" file name.
    pub artist: String,
    pub title: String,
    /// The song's playlist; empty uses [`PackOptions::playlist`].
    pub playlist: String,
    pub seconds: f64,
    /// Why the song may not pack; empty means fine.
    pub problems: Vec<String>,
    /// A chosen cover image; otherwise the embedded album art is used, if any.
    pub artwork: Option<PathBuf>,
    /// Detected by [`scan`]; a chosen image overrides it.
    pub has_embedded_artwork: bool,
}

impl SongInfo {
    /// "Artist - Title": how the music menu and reskate-music.json identify the song.
    pub fn id(&self) -> String {
        format!("{} - {}", self.artist, self.title)
    }
}

#[derive(Clone, Debug)]
pub struct PackOptions {
    /// The folder with Skate.exe.
    pub game: PathBuf,
    pub output: PathBuf,
    /// The mod's name.
    pub name: String,
    /// The default playlist in the music menu.
    pub playlist: String,
    /// Opus bitrate in kbps, 64-320.
    pub bitrate: u32,
    /// EBU R128 loudness normalisation (two-pass to -15 LUFS) via ffmpeg's loudnorm.
    pub normalize: bool,
    pub playlist_artwork: BTreeMap<String, PathBuf>,
    /// Playlists that get a generated text cover; a chosen image takes priority.
    pub generated_playlist_artwork: BTreeSet<String>,
}

impl Default for PackOptions {
    fn default() -> Self {
        PackOptions {
            game: PathBuf::new(),
            output: PathBuf::new(),
            name: "ReSkateMusic".into(),
            playlist: String::new(),
            bitrate: 192,
            normalize: true,
            playlist_artwork: BTreeMap::new(),
            generated_playlist_artwork: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// A song is being encoded.
    Encoding,
    /// A song is done; the detail gives its length, packets and size.
    Encoded,
    /// The bundle and TOC are being assembled (whole mod; `song == count`).
    Building,
    /// Files are being written (whole mod; `song == count`).
    Writing,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Encoding => "encoding",
            Stage::Encoded => "encoded",
            Stage::Building => "building",
            Stage::Writing => "writing",
        }
    }
}

/// One step of [`pack`].
#[derive(Clone, Debug)]
pub struct Progress {
    pub song: usize,
    pub count: usize,
    pub stage: Stage,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct PackResult {
    pub songs: usize,
    pub audio_kb: usize,
    pub output: PathBuf,
    /// Whether the EBX payloads were Oodle-compressed (only with the game's codec, on Windows).
    pub compressed: bool,
}
