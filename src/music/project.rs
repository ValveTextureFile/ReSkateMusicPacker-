// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! reskate-music-project.json: what a mod was made from (source files as absolute paths), so it
//! can be reopened, changed and packed again into the same folder.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::{PackOptions, SongInfo};
use crate::{Error, Result};

pub const PROJECT_FILE: &str = "reskate-music-project.json";

#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub playlist: String,
    pub bitrate: u32,
    pub normalize: bool,
    /// File, artist, title, playlist and artwork; scan the files for length and problems.
    pub songs: Vec<SongInfo>,
    pub playlist_artwork: BTreeMap<String, PathBuf>,
    pub generated_playlist_artwork: BTreeSet<String>,
}

/// Object keys in sorted order at every level, as the C++ tool wrote them, whatever map type
/// serde_json was built with.
pub fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(entries.into_iter().map(|(key, value)| (key, canonical(value))).collect())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        other => other,
    }
}

pub fn pretty(value: Value) -> String {
    serde_json::to_string_pretty(&canonical(value)).expect("JSON values serialize") + "\n"
}

fn absolute(path: &Path) -> String {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned()
}

pub(super) fn to_json(options: &PackOptions, songs: &[SongInfo]) -> String {
    let playlist_artwork: Map<String, Value> = options
        .playlist_artwork
        .iter()
        .filter(|(_, path)| !path.as_os_str().is_empty())
        .map(|(name, path)| (name.clone(), Value::String(absolute(path))))
        .collect();
    let songs: Vec<Value> = songs
        .iter()
        .map(|song| {
            let mut entry = Map::new();
            entry.insert("file".into(), json!(absolute(&song.file)));
            entry.insert("artist".into(), json!(song.artist));
            entry.insert("title".into(), json!(song.title));
            if !song.playlist.is_empty() {
                entry.insert("playlist".into(), json!(song.playlist));
            }
            if let Some(artwork) = &song.artwork {
                entry.insert("artwork".into(), json!(absolute(artwork)));
            }
            Value::Object(entry)
        })
        .collect();
    pretty(json!({
        "schema": 1,
        "name": options.name,
        "playlist": options.playlist,
        "bitrate": options.bitrate,
        "normalize": options.normalize,
        "playlist_artwork": playlist_artwork,
        "generated_playlist_artwork": options.generated_playlist_artwork,
        "songs": songs,
    }))
}

fn parse(root: &Value) -> std::result::Result<Project, String> {
    let string = |value: &Value, key: &str| -> std::result::Result<String, String> {
        value.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| format!("Missing JSON field: {key}"))
    };
    if root.get("schema").and_then(Value::as_i64).unwrap_or(0) != 1 {
        return Err("unsupported schema".into());
    }
    let bitrate = root.get("bitrate").and_then(Value::as_u64).ok_or("Missing JSON field: bitrate")?;
    let mut project = Project {
        name: string(root, "name")?,
        playlist: string(root, "playlist")?,
        bitrate: bitrate as u32,
        normalize: root.get("normalize").and_then(Value::as_bool).unwrap_or(true),
        songs: Vec::new(),
        playlist_artwork: BTreeMap::new(),
        generated_playlist_artwork: BTreeSet::new(),
    };
    if let Some(art) = root.get("playlist_artwork").and_then(Value::as_object) {
        for (name, path) in art {
            let path = path.as_str().ok_or("playlist artwork must be a path")?;
            project.playlist_artwork.insert(name.clone(), PathBuf::from(path));
        }
    }
    if let Some(names) = root.get("generated_playlist_artwork").and_then(Value::as_array) {
        for name in names {
            project.generated_playlist_artwork.insert(name.as_str().ok_or("playlist names must be text")?.to_owned());
        }
    }
    let songs = root.get("songs").and_then(Value::as_array).ok_or("Missing JSON field: songs")?;
    for song in songs {
        project.songs.push(SongInfo {
            file: PathBuf::from(string(song, "file")?),
            artist: string(song, "artist")?,
            title: string(song, "title")?,
            playlist: song.get("playlist").and_then(Value::as_str).unwrap_or_default().to_owned(),
            artwork: song.get("artwork").and_then(Value::as_str).map(PathBuf::from),
            ..Default::default()
        });
    }
    Ok(project)
}

/// Reads the project a mod was built from. Fails, with a message fit to show, when the folder has
/// no project or a bad one.
pub fn load_project(folder: &Path) -> Result<Project> {
    let path = folder.join(PROJECT_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        let name = folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(Error::msg(format!(
            "{name} has no project file (it was made before they existed); make it again from its song files"
        )));
    };
    let parsed = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).map_err(|error| error.to_string());
    parsed
        .and_then(|root| parse(&root))
        .map_err(|error| Error::msg(format!("{} is not a valid project: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_round_trip() {
        let options = PackOptions {
            name: "Mix".into(),
            playlist: "Tones".into(),
            bitrate: 224,
            normalize: false,
            playlist_artwork: [("Tones".to_string(), PathBuf::from("/covers/a.png"))].into(),
            generated_playlist_artwork: ["Second".to_string()].into(),
            ..Default::default()
        };
        let songs = vec![
            SongInfo { file: "/music/a.mp3".into(), artist: "A".into(), title: "One".into(), ..Default::default() },
            SongInfo {
                file: "/music/b.flac".into(),
                artist: "B".into(),
                title: "Two".into(),
                playlist: "Second".into(),
                artwork: Some("/covers/b.png".into()),
                ..Default::default()
            },
        ];
        let text = to_json(&options, &songs);
        assert!(text.starts_with("{\n  \"bitrate\": 224,\n"));
        let project = parse(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(
            (project.name.as_str(), project.playlist.as_str(), project.bitrate, project.normalize),
            ("Mix", "Tones", 224, false)
        );
        assert_eq!(project.songs.len(), 2);
        assert_eq!(project.songs[1].playlist, "Second");
        assert!(project.songs[1].artwork.is_some() && project.songs[0].artwork.is_none());
        assert_eq!(project.generated_playlist_artwork, options.generated_playlist_artwork);
        assert!(project.playlist_artwork.contains_key("Tones"));
    }

    #[test]
    fn legacy_projects_load_without_artwork() {
        let root = json!({"schema": 1, "name": "M", "playlist": "P", "bitrate": 192,
            "songs": [{"file": "/a.mp3", "artist": "A", "title": "T"}]});
        let project = parse(&root).unwrap();
        assert!(project.normalize && project.playlist_artwork.is_empty() && project.songs[0].artwork.is_none());
        assert!(parse(&json!({"schema": 2})).is_err());
    }
}
