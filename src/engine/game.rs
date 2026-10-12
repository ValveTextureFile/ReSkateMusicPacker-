// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-FileCopyrightText: 2026 the ReSkate contributors
// SPDX-License-Identifier: GPL-3.0-only
// Ported from the ReSkate project (https://github.com/Dingo-Shenanigans/ReSkate); see NOTICE.md.
//! Reading individual assets out of the installed game's bundles, without loading the whole
//! asset database. Archives live at `<Data>/Win32/<package>/cas_NN.cas`, with each install
//! chunk's package directory taken from `layout.toc`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::bundle::{
    AssetKind, BinaryBundle, BundleAsset, BundleFileInfo, CasIdentifier, read_binary_bundle, read_bundle_region,
};
use super::cas::{DecodeOptions, decode_cas};
use super::native_db::{self, ENVELOPE_SIZE, MAGIC, Node};
use super::toc::{TocDocument, read_toc};
use crate::error::IoContext;
use crate::{Result, bail};

/// "cas_NN.cas"
pub fn archive_file(archive: u16) -> String {
    format!("cas_{archive:02}.cas")
}

/// The number at the end of a cas file's stem ("cas_01" -> 1).
pub fn archive_index(stem: &str) -> Option<u16> {
    let digits = stem.len() - stem.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits == stem.len() {
        return None;
    }
    stem[digits..].parse::<u32>().ok().and_then(|value| u16::try_from(value).ok())
}

/// Each record of layout.toc's install chunk file tables: a little-endian archive index followed
/// by the install chunk id it belongs to.
fn install_chunk_files(table: &[u8]) -> impl Iterator<Item = (u32, u16)> + '_ {
    table.as_chunks::<8>().0.iter().map(|record| {
        let archive = u16::from_le_bytes([record[0], record[1]]);
        let id = u32::from_le_bytes([record[2], record[3], record[4], record[5]]);
        (id, archive)
    })
}

/// Parses a layout.toc file, envelope included.
pub fn read_layout(path: &Path) -> Result<Node> {
    let buffer = std::fs::read(path).at("Cannot read", path)?;
    if buffer.len() <= ENVELOPE_SIZE || buffer[..4] != MAGIC {
        bail!("Unrecognized native envelope in {}", path.display());
    }
    native_db::read(&buffer[ENVELOPE_SIZE..], "layout.toc", native_db::Options { unique_fields: false })
}

fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            walk(&entry.path(), found);
        } else if kind.is_file() {
            found.push(entry.path());
        }
    }
}

pub struct GameArchives {
    /// A data layer's directory (the game's Data folder).
    root: PathBuf,
    /// Install chunk -> package directory, lower case as the manifest names it.
    chunk_directory: BTreeMap<u32, String>,
    /// Lower-case package directory -> its spelling on disk, relative to Win32.
    on_disk: HashMap<String, String>,
}

impl GameArchives {
    pub fn new(root: PathBuf, layout: &Node) -> Self {
        let win32 = root.join("Win32");
        let mut files = Vec::new();
        walk(&win32, &mut files);
        let mut directories: BTreeMap<String, BTreeSet<u16>> = BTreeMap::new();
        for file in files {
            let is_cas = file.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("cas"));
            let Some(index) = file.file_stem().and_then(|stem| stem.to_str()).and_then(archive_index) else { continue };
            let Some(relative) = file.parent().and_then(|parent| parent.strip_prefix(&win32).ok()) else { continue };
            if !is_cas {
                continue;
            }
            let relative =
                relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            directories.entry(relative).or_default().insert(index);
        }
        let on_disk = directories.keys().map(|directory| (directory.to_lowercase(), directory.clone())).collect();
        let mut by_archives: BTreeMap<&BTreeSet<u16>, Vec<&String>> = BTreeMap::new();
        for (directory, archives) in &directories {
            by_archives.entry(archives).or_default().push(directory);
        }

        // Each install chunk in the manifest names its package directory, and that is where its
        // archives live, whatever set of cas files a given installation happens to hold.
        let mut chunk_directory = BTreeMap::new();
        let chunks = layout.field("installManifest").and_then(|manifest| manifest.field("installChunks"));
        for entry in chunks.map(|chunks| chunks.children.as_slice()).unwrap_or_default() {
            let (Some(name), Some(index)) = (entry.field("name"), entry.field("persistentIndex")) else { continue };
            if name.kind != 7 || name.text.is_empty() || index.kind != 8 || index.bytes.len() != 4 {
                continue;
            }
            let id = u32::from_le_bytes(index.bytes[..4].try_into().expect("4 bytes"));
            chunk_directory.entry(id).or_insert_with(|| name.text.to_lowercase());
        }

        // Chunks the manifest does not name are matched by their archives. A chunk is only
        // resolvable when one directory holds exactly the archives it declares; the per-language
        // folders all hold {1} and stay ambiguous, which is fine because no mod references them.
        for field in ["layeredInstallChunkFiles", "unlayeredInstallChunkFiles"] {
            let Some(node) = layout.field(field).filter(|node| node.kind == 19) else { continue };
            let mut declared: BTreeMap<u32, BTreeSet<u16>> = BTreeMap::new();
            for (id, archive) in install_chunk_files(&node.bytes) {
                declared.entry(id).or_default().insert(archive);
            }
            for (id, archives) in declared {
                if let Some([only]) = by_archives.get(&archives).map(Vec::as_slice) {
                    chunk_directory.entry(id).or_insert_with(|| only.to_lowercase());
                }
            }
        }
        GameArchives { root, chunk_directory, on_disk }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn directory(&self, install_chunk: u32) -> Result<&str> {
        match self.chunk_directory.get(&install_chunk) {
            Some(directory) => Ok(self.on_disk.get(directory).unwrap_or(directory)),
            None => bail!("No package directory matches install chunk {install_chunk}"),
        }
    }

    pub fn read(&self, location: &CasIdentifier, offset: u32, size: u32) -> Result<Vec<u8>> {
        let path =
            self.root.join("Win32").join(self.directory(location.install_chunk)?).join(archive_file(location.archive));
        let mut file = File::open(&path).at("Cannot open", &path)?;
        file.seek(SeekFrom::Start(offset as u64)).at("Cannot read", &path)?;
        let mut bytes = vec![0u8; size as usize];
        file.read_exact(&mut bytes).at("Cannot read", &path)?;
        Ok(bytes)
    }
}

pub struct GameBundle {
    pub manifest: BinaryBundle,
    /// Where each asset's payload lives: the manifest's ebx, then resources, then chunks.
    pub payloads: Vec<BundleFileInfo>,
}

impl GameBundle {
    fn assets(&self, kind: AssetKind) -> &[BundleAsset] {
        match kind {
            AssetKind::Ebx => &self.manifest.ebx,
            AssetKind::Resource => &self.manifest.resources,
            AssetKind::Chunk => &self.manifest.chunks,
        }
    }

    pub fn payload(&self, kind: AssetKind, index: usize) -> Option<&BundleFileInfo> {
        if index >= self.assets(kind).len() {
            return None;
        }
        let first = match kind {
            AssetKind::Ebx => 0,
            AssetKind::Resource => self.manifest.ebx.len(),
            AssetKind::Chunk => self.manifest.ebx.len() + self.manifest.resources.len(),
        };
        self.payloads.get(first + index)
    }

    /// The asset named `name` and its index among assets of its kind.
    pub fn find(&self, kind: AssetKind, name: &str) -> Option<(usize, &BundleAsset)> {
        self.assets(kind).iter().enumerate().find(|(_, asset)| asset.name == name.as_bytes())
    }
}

pub struct GameData {
    game_root: PathBuf,
    archives: GameArchives,
}

impl GameData {
    /// `game_root` is the folder with Skate.exe; its Data folder is read.
    pub fn open(game_root: &Path) -> Result<Self> {
        let data = game_root.join("Data");
        let layout = read_layout(&data.join("layout.toc"))?;
        Ok(GameData { game_root: game_root.to_path_buf(), archives: GameArchives::new(data, &layout) })
    }

    pub fn read_toc(&self, relative: &str) -> Result<TocDocument> {
        let path = self.game_root.join("Data").join(relative);
        let bytes = std::fs::read(&path).at("Cannot read", &path)?;
        read_toc(&bytes, &[])
    }

    /// The bundle named `name` (lower case) in a superbundle TOC.
    pub fn read_bundle(&self, toc: &TocDocument, name: &str) -> Result<Option<GameBundle>> {
        let Some(bundle) = toc.bundles.iter().find(|bundle| bundle.name == name) else { return Ok(None) };
        let region = read_bundle_region(&bundle.region)?;
        if !region.inline_manifest.is_empty() {
            return Ok(Some(GameBundle {
                manifest: read_binary_bundle(&region.inline_manifest)?,
                payloads: region.files,
            }));
        }
        // The manifest is the region's first file, in cas like any payload. It is usually stored
        // raw, so that comes first and the block decoder is the fallback.
        let Some(first) = region.files.first() else { bail!("Bundle {name} has no manifest") };
        let raw = self.archives.read(&first.location, first.offset, first.size)?;
        let manifest = match read_binary_bundle(&raw) {
            Ok(manifest) => manifest,
            Err(_) => read_binary_bundle(&decode_cas(&raw, &DecodeOptions::new(&self.game_root))?)?,
        };
        Ok(Some(GameBundle { manifest, payloads: region.files[1..].to_vec() }))
    }

    /// A payload, decoded.
    pub fn read(&self, payload: &BundleFileInfo) -> Result<Vec<u8>> {
        let raw = self.archives.read(&payload.location, payload.offset, payload.size)?;
        decode_cas(&raw, &DecodeOptions::new(&self.game_root))
    }

    pub fn archives(&self) -> &GameArchives {
        &self.archives
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_index_needs_a_prefix_and_trailing_digits() {
        assert_eq!(archive_index("cas_01"), Some(1));
        assert_eq!(archive_index("cas_65535"), Some(65535));
        assert_eq!(archive_index("cas_65536"), None);
        assert_eq!(archive_index("cas_"), None);
        assert_eq!(archive_index("01"), None);
        assert_eq!(archive_file(1), "cas_01.cas");
    }
}
