// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The remembered game folder and ffmpeg folder, in the user's config folder
//! (`%APPDATA%\ReSkateMusicPacker\settings.json` on Windows, as before).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::Result;
use crate::music::tools::write_file;

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub game: PathBuf,
    pub ffmpeg: PathBuf,
}

pub fn settings_file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ReSkateMusicPacker").join("settings.json"))
}

impl Settings {
    /// The saved settings, or defaults when there are none or they cannot be read.
    pub fn load() -> Settings {
        let root = settings_file()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let path = |key: &str| {
            root.as_ref().and_then(|root| root.get(key)).and_then(Value::as_str).map(PathBuf::from).unwrap_or_default()
        };
        Settings { game: path("game"), ffmpeg: path("ffmpeg") }
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = settings_file() else { return Ok(()) };
        let text = serde_json::to_string_pretty(&json!({
            "ffmpeg": self.ffmpeg.to_string_lossy(),
            "game": self.game.to_string_lossy(),
        }))
        .expect("JSON values serialize");
        write_file(&path, format!("{text}\n").as_bytes())
    }
}

/// Whether `folder` is a game install: it holds Skate.exe.
pub fn is_game_folder(folder: &std::path::Path) -> bool {
    !folder.as_os_str().is_empty() && folder.join("Skate.exe").is_file()
}
