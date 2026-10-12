// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Thunderstore packages: a built mod as a .zip with Thunderstore's manifest.json, a generated
//! README.md tracklist and a 256x256 icon.png.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::artwork::{ICON_FILTER, default_icon_png, ffmpeg_png};
use super::project::{PROJECT_FILE, load_project, pretty};
use super::tools;
use crate::error::IoContext;
use crate::{Error, Result, bail};

#[derive(Clone, Debug)]
pub struct ThunderstoreOptions {
    pub author: String,
    pub version: String,
    pub description: String,
    /// A custom icon; otherwise the mod's own icon.png, else a generated one.
    pub icon: Option<PathBuf>,
    /// The .zip path, or a folder for it; by default `<Author>-<Name>-<Version>.zip` beside the mod.
    pub output: Option<PathBuf>,
    /// Include the "Packaged with ReSkate Music Packer" link in the README.
    pub readme_credit: bool,
}

impl Default for ThunderstoreOptions {
    fn default() -> Self {
        ThunderstoreOptions {
            author: "Author".into(),
            version: "1.0.0".into(),
            description: String::new(),
            icon: None,
            output: None,
            readme_credit: true,
        }
    }
}

/// Thunderstore names: letters, digits and underscores ("My Mix" -> My_Mix).
pub fn thunderstore_name(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if c == ' ' || c == '-' {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_');
    if out.is_empty() { "Mod".into() } else { out.into() }
}

fn mod_name(folder: &Path) -> String {
    if folder.join(PROJECT_FILE).exists() {
        if let Ok(project) = load_project(folder)
            && !project.name.is_empty()
        {
            return project.name;
        }
    } else if let Ok(text) = std::fs::read_to_string(folder.join("manifest.json"))
        && let Ok(root) = serde_json::from_str::<Value>(&text)
        && let Some(name) = root.get("name").and_then(Value::as_str)
    {
        return name.to_owned();
    }
    "ReSkateMusic".into()
}

fn readme(name: &str, description: &str, folder: &Path, credit: bool) -> String {
    let mut md = format!("# {name}\n\n");
    if description.is_empty() {
        md.push_str("Adds custom music to skate. through ReSkate.\n\n");
    } else {
        md.push_str(&format!("{description}\n\n"));
    }
    let music = std::fs::read_to_string(folder.join("reskate-music.json")).ok();
    if let Some(Value::Array(playlists)) =
        music.and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|root| root.get("playlists").cloned())
    {
        md.push_str("## Playlists & Tracklist\n\n");
        for playlist in &playlists {
            let title = playlist.get("name").and_then(Value::as_str).unwrap_or("Playlist");
            md.push_str(&format!("### {title}\n\n"));
            if let Some(songs) = playlist.get("songs").and_then(Value::as_array) {
                for song in songs.iter().filter_map(Value::as_str) {
                    md.push_str(&format!("- {song}\n"));
                }
                md.push('\n');
            }
        }
    }
    md.push_str("## Installation\n\nInstall via Thunderstore Mod Manager, or drop into your ReSkate launcher.\n");
    if credit {
        md.push_str(
            "\n---\n*Packaged with [ReSkate Music Packer](https://github.com/DeckardDetribine/ReSkateMusicPacker)*\n",
        );
    }
    md
}

fn icon(options: &ThunderstoreOptions, folder: &Path) -> Result<Vec<u8>> {
    if let Some(custom) = options.icon.as_ref().filter(|path| path.is_file()) {
        let temporary = tools::scratch_file("icon", "png")?;
        let converted = ffmpeg_png(custom, None, ICON_FILTER, &temporary)
            .ok()
            .and_then(|()| std::fs::read(&temporary).ok())
            .filter(|bytes| !bytes.is_empty());
        let _ = std::fs::remove_file(&temporary);
        return match converted {
            Some(bytes) => Ok(bytes),
            None => std::fs::read(custom).at("Cannot read", custom),
        };
    }
    let own = folder.join("icon.png");
    if own.is_file() {
        return std::fs::read(&own).at("Cannot read", &own);
    }
    default_icon_png()
}

/// Every file under `folder`, as (path, zip name), sorted by name so packages are reproducible.
fn mod_files(folder: &Path) -> Result<Vec<(PathBuf, String)>> {
    fn walk(directory: &Path, prefix: &str, top: bool, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
        for entry in std::fs::read_dir(directory).at("Cannot read", directory)? {
            let entry = entry.at("Cannot read", directory)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Thunderstore's own files replace the mod's; the project file holds local paths.
            let skipped = ["manifest.json", "icon.png", "readme.md", PROJECT_FILE];
            if top && skipped.contains(&name.to_lowercase().as_str()) {
                continue;
            }
            let path = entry.path();
            let zip_name = format!("{prefix}{name}");
            if entry.file_type().at("Cannot read", &path)?.is_dir() {
                walk(&path, &format!("{zip_name}/"), false, out)?;
            } else {
                out.push((path, zip_name));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(folder, "", true, &mut files)?;
    files.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(files)
}

/// Packages a built mod folder as a Thunderstore-compatible .zip and returns its path.
pub fn export_thunderstore(folder: &Path, options: &ThunderstoreOptions) -> Result<PathBuf> {
    if !folder.join("layout.toc").exists() {
        bail!("Folder is not a built ReSkate mod (missing layout.toc): {}", folder.display());
    }
    let author = thunderstore_name(if options.author.is_empty() { "Author" } else { &options.author });
    let name = thunderstore_name(&mod_name(folder));
    let version = if options.version.matches('.').count() == 2 { options.version.clone() } else { "1.0.0".into() };
    let mut description = if options.description.is_empty() {
        "Adds music to skate. through ReSkate.".to_owned()
    } else {
        options.description.clone()
    };
    if description.len() > 250 {
        let mut end = 250;
        while !description.is_char_boundary(end) {
            end -= 1;
        }
        description.truncate(end);
    }

    let zip_name = format!("{author}-{name}-{version}.zip");
    let output = match &options.output {
        None => folder.parent().unwrap_or(folder).join(&zip_name),
        Some(path) if path.is_dir() => path.join(&zip_name),
        Some(path) => path.clone(),
    };
    let manifest = pretty(json!({
        "name": name,
        "version_number": version,
        "website_url": "",
        "description": description,
        "dependencies": [],
    }));
    let readme = readme(&name, &description, folder, options.readme_credit);
    let icon = icon(options, folder)?;
    let files = mod_files(folder)?;

    // Written beside the target first, so a failed export never leaves a broken package.
    let mut partial_name = output.file_name().unwrap_or_default().to_os_string();
    partial_name.push(".partial");
    let partial = output.with_file_name(partial_name);
    let write = || -> Result<()> {
        let file = File::create(&partial).at("Cannot write", &partial)?;
        let mut zip = ZipWriter::new(file);
        let entry = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let failure =
            |error: zip::result::ZipError| Error::msg(format!("Could not build the package archive: {error}"));
        let output_absolute = std::path::absolute(&output).ok();
        for (path, zip_name) in &files {
            if std::path::absolute(path).ok() == output_absolute {
                continue;
            }
            zip.start_file(zip_name.as_str(), entry).map_err(failure)?;
            let mut source = File::open(path).at("Cannot read", path)?;
            std::io::copy(&mut source, &mut zip).at("Cannot read", path)?;
        }
        for (zip_name, bytes) in
            [("manifest.json", manifest.as_bytes()), ("README.md", readme.as_bytes()), ("icon.png", &icon)]
        {
            zip.start_file(zip_name, entry).map_err(failure)?;
            zip.write_all(bytes).map_err(|error| Error::io("Could not build the package archive", error))?;
        }
        zip.finish().map_err(failure)?;
        Ok(())
    };
    if let Err(error) = write() {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    let _ = std::fs::remove_file(&output);
    std::fs::rename(&partial, &output).at("Cannot write", &output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_letters_digits_and_underscores() {
        assert_eq!(thunderstore_name("My Mix - Vol.2"), "My_Mix___Vol2");
        assert_eq!(thunderstore_name("!!!"), "Mod");
        assert_eq!(thunderstore_name("Tail "), "Tail");
    }
}
