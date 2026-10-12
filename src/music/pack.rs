// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Builds the mod: a folder of songs -> a mod that ADDS them to the game's music. Nothing shipped
//! is replaced; every song gets its own assets and audio.
//!
//! What one song is, in the game's data (all in bam_levelroot/bam_coregameassets):
//!   <slug>_MG   MusicGraphAsset, cloned from a shipped song: name, NameHash, artist/title and
//!               its wave import change; every instance gets a fresh GUID.
//!   <slug>_NWA  NewWaveAsset, cloned from that song's wave: its two chunk ids and sizes.
//!   two TOC chunks: a prefetch chunk (22-byte header + the stream's seek table) and the stream
//!               (EA blocks: one H header, one D block per Opus packet, one E end).
//!   an entry in Audio/Music/Playlist/DGO_MUS_Playlist's Assets: the audio engine only plays
//!   songs listed there; registration alone makes a song listable, not playable.
//! The bundle manifest is the region's first file, stored raw in cas like the game's own: the
//! game crashes on a bundle whose manifest is inline in the TOC.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value as Json, json};

use super::artwork::{embedded_artwork, image_artwork_png, playlist_artwork_png};
use super::audio::{self, codec_header, header_samples, playable_samples, prefetch_chunk, stream_chunk};
use super::text::{sanitize_text, slug_of, unique_slug, usable_name};
use super::tools::{self, write_file};
use super::wave::{WAVE_RESOURCE_TYPE, WaveFacts, name_hash, wave_resource};
use super::{PackOptions, PackResult, Progress, SongInfo, Stage, project};
use crate::engine::binary::{Guid, Sha1};
use crate::engine::bundle::{
    AssetKind, BundleFileInfo, CasIdentifier, read_bundle_region, write_binary_bundle, write_bundle_region,
};
use crate::engine::cas::{CasCompression, EncodeOptions, encode_cas};
use crate::engine::ebx::{self, Document, PointerKind, PointerReference, Value};
use crate::engine::game::GameData;
use crate::engine::oodle;
use crate::engine::toc::{TocBundle, TocChunk, read_toc, verify_toc, write_patch_toc};
use crate::error::IoContext;
use crate::{Error, Result, bail};

const TOC_PATH: &str = "Win32/levels/game/bam_levelroot/bam_levelroot.toc";
const BUNDLE_NAME: &str = "win32/levels/game/bam_levelroot/bam_coregameassets";
const PLAYLIST_ASSET: &str = "audio/music/playlist/dgo_mus_playlist";
/// The template song and the wave its music segment plays. Belly Dancer: one music segment, one
/// silent lead-in, 48 kHz stereo Opus like every shipped song.
const TEMPLATE_SONG: &str = "audio/music/assets/turkish/turkish_upm_belly_dancer_mg";
const TEMPLATE_WAVE: &str = "audio/music/assets/turkish/turkish_upm_belly_dancer_chapwr13a_1_nwa";
const ASSET_FOLDER: &str = "Audio/Music/Assets/ReSkate/";
/// Where the shipped bundle's manifest lives (defaultinstallpackage); the mod's archive goes beside it.
const INSTALL_CHUNK: u32 = 2_016_875_820;
const CAS_RELATIVE: &str = "Win32/configurations/layout/defaultinstallpackage/cas_01.cas";
/// The tool + schema marker ReSkate reads; its version is the format's, not this program's.
const BUILD_MARKER: &str = "{\n  \"schema\": 1,\n  \"tool\": \"ReSkateMusicPacker\",\n  \"version\": \"0.1.0\"\n}\n";

const MOD_ARCHIVE: CasIdentifier = CasIdentifier { patch: true, install_chunk: INSTALL_CHUNK, archive: 1 };

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { Ok(()) }
}

/// Sets an integer field, keeping its signedness.
fn set_number(value: &mut Value, number: u64) {
    *value = match value {
        Value::Int(_) => Value::Int(number as i64),
        _ => Value::UInt(number),
    };
}

fn number(value: &Value) -> Result<u64> {
    match value {
        Value::UInt(number) => Ok(*number),
        Value::Int(number) => Ok(*number as u64),
        _ => bail!("Expected a number in the template"),
    }
}

fn array_mut(value: &mut Value) -> Result<&mut Vec<Value>> {
    match value {
        Value::Array(values) => Ok(values),
        _ => bail!("Expected an array in the template"),
    }
}

fn object_mut(value: &mut Value) -> Result<&mut ebx::Object> {
    match value {
        Value::Object(object) => Ok(object),
        _ => bail!("Expected an object in the template"),
    }
}

/// A fresh partition: new file GUID and instance GUIDs. Internal pointers are by index.
fn renew(document: &mut Document) {
    document.file_guid = Guid::random();
    for instance in &mut document.instances {
        instance.instance_guid = Guid::random();
    }
}

fn text(value: &str) -> Value {
    Value::Bytes(value.as_bytes().to_vec())
}

/// A song checked and named, ready to encode.
struct Song<'a> {
    info: &'a SongInfo,
    artist: String,
    title: String,
    slug: String,
    playlist: String,
}

impl Song<'_> {
    fn id(&self) -> String {
        format!("{} - {}", self.artist, self.title)
    }
}

/// A built asset: its lower-case bundle name and its decoded payload.
struct Built {
    name: String,
    payload: Vec<u8>,
}

/// Builds the mod into `options.output`; files already there are overwritten. Fails with a
/// message fit to show, or [`Error::Cancelled`] when `cancel` was set (checked between songs), in
/// which case nothing is left at the output folder. Songs keep their order; their artist and title
/// are used as given.
pub fn pack(
    options: &PackOptions,
    songs: &[SongInfo],
    progress: &mut dyn FnMut(&Progress),
    cancel: &AtomicBool,
) -> Result<PackResult> {
    if songs.is_empty() {
        bail!("No songs to pack");
    }
    if !usable_name(&options.playlist) {
        bail!("The playlist name is empty, too long, or has control characters");
    }
    // Build beside the output and move it into place at the end, so a failure or a cancel leaves
    // no half-written mod.
    let out = &options.output;
    let mut partial_name = out.file_name().ok_or_else(|| Error::msg("The output folder has no name"))?.to_os_string();
    partial_name.push(".partial");
    let partial = out.with_file_name(partial_name);
    let _ = std::fs::remove_dir_all(&partial);
    let result = build(options, &partial, songs, progress, cancel).and_then(|mut result| {
        check(cancel)?;
        if out.exists() {
            // An existing mod folder is updated in place, so files the user put there (an
            // icon.png, say) survive a rebuild.
            copy_tree(&partial, out)?;
            let _ = std::fs::remove_dir_all(&partial);
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).at("Cannot create", parent)?;
            }
            std::fs::rename(&partial, out).at("Cannot move the mod into", out)?;
        }
        result.output = out.clone();
        Ok(result)
    });
    if result.is_err() && partial.exists() {
        let _ = std::fs::remove_dir_all(&partial);
    }
    result
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).at("Cannot create", to)?;
    for entry in std::fs::read_dir(from).at("Cannot read", from)? {
        let entry = entry.at("Cannot read", from)?;
        let target = to.join(entry.file_name());
        if entry.file_type().at("Cannot read", &entry.path())?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).at("Cannot write", &target)?;
        }
    }
    Ok(())
}

/// The template wave's facts and its prefetch chunk.
fn template_facts(data: &GameData, toc_chunks: &[TocChunk], wave_bytes: &[u8]) -> Result<(WaveFacts, Vec<u8>)> {
    let mut wave = ebx::read_document(wave_bytes)?;
    let root = wave.instance_of("NewWaveAsset")?;
    let instance = root.instance_guid;
    let object = root.object.as_mut().expect("instance_of returns instances with objects");
    let chunks = array_mut(object.field_mut("Chunks")?)?;
    if chunks.len() != 2 {
        bail!("Template wave does not have two chunks");
    }
    let mut chunk_at = |k: usize| -> Result<(Guid, u32)> {
        let entry = object_mut(&mut chunks[k])?;
        let Value::Guid(id) = *entry.field("ChunkId")? else { bail!("Template wave chunk has no id") };
        Ok((id, number(entry.field("ChunkSize")?)? as u32))
    };
    let (prefetch_id, prefetch_size) = chunk_at(0)?;
    let (stream_id, stream_size) = chunk_at(1)?;
    let mut prefetch = Vec::new();
    for chunk in toc_chunks.iter().filter(|chunk| chunk.guid == prefetch_id) {
        prefetch = data.read(&BundleFileInfo { location: chunk.location, offset: chunk.offset, size: chunk.size })?;
    }
    if prefetch.len() != prefetch_size as usize || prefetch.len() < 22 {
        bail!("Template prefetch chunk not found");
    }
    let header: [u8; 10] = prefetch[12..22].try_into().expect("10 bytes");
    let facts = WaveFacts {
        instance,
        prefetch_id,
        stream_id,
        prefetch_size,
        stream_size,
        samples: header_samples(&header),
        name: String::new(),
    };
    Ok((facts, prefetch))
}

/// Builds the whole mod into `out`, an empty folder.
fn build(
    options: &PackOptions,
    out: &Path,
    files: &[SongInfo],
    progress: &mut dyn FnMut(&Progress),
    cancel: &AtomicBool,
) -> Result<PackResult> {
    let count = files.len();
    let mut report = |song: usize, stage: Stage, detail: String| progress(&Progress { song, count, stage, detail });

    let data = GameData::open(&options.game)?;
    let toc = data.read_toc(TOC_PATH)?;
    let bundle =
        data.read_bundle(&toc, BUNDLE_NAME)?.ok_or_else(|| Error::msg("The game has no bam_coregameassets bundle"))?;
    let shipped = toc.bundles.iter().find(|bundle| bundle.name == BUNDLE_NAME).expect("read_bundle found it");
    let shipped_region = read_bundle_region(&shipped.region)?;
    if shipped_region.files.first().map(|file| file.location.install_chunk) != Some(INSTALL_CHUNK) {
        bail!("bam_coregameassets is not where this tool expects; is this the supported game build?");
    }

    let ebx_of = |name: &str| -> Result<(usize, Vec<u8>)> {
        let (index, _) =
            bundle.find(AssetKind::Ebx, name).ok_or_else(|| Error::msg(format!("The game has no {name}")))?;
        let payload =
            bundle.payload(AssetKind::Ebx, index).ok_or_else(|| Error::msg(format!("The game has no {name}")))?;
        Ok((index, data.read(payload)?))
    };
    let (song_index, song_bytes) = ebx_of(TEMPLATE_SONG)?;
    let (_, wave_bytes) = ebx_of(TEMPLATE_WAVE)?;
    let (playlist_index, playlist_bytes) = ebx_of(PLAYLIST_ASSET)?;
    let mut playlist = ebx::read_document(&playlist_bytes)?;
    let template_wave_file = ebx::read_document(&wave_bytes)?.file_guid;
    let (template_facts, template_prefetch) = template_facts(&data, &toc.chunks, &wave_bytes)?;
    let template_header: [u8; 10] = template_prefetch[12..22].try_into().expect("10 bytes");
    let (template_resource_index, template_resource) = bundle
        .find(AssetKind::Resource, TEMPLATE_WAVE)
        .filter(|(_, resource)| resource.resource_type == WAVE_RESOURCE_TYPE)
        .ok_or_else(|| Error::msg("The template wave has no sound-bank resource"))?;
    let template_resource = template_resource.clone();
    let template_blob = data.read(
        bundle
            .payload(AssetKind::Resource, template_resource_index)
            .ok_or_else(|| Error::msg("The template wave has no sound-bank resource"))?,
    )?;

    // Check every song before encoding any: a bad name should not cost minutes of encoding first.
    let mut ids = BTreeSet::new();
    let mut slugs = BTreeSet::new();
    let mut songs = Vec::with_capacity(count);
    for info in files {
        let file_name = info.file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        if !usable_name(&info.artist) || !usable_name(&info.title) {
            bail!("{file_name}: artist or title is empty, too long, or has control characters");
        }
        let playlist = if info.playlist.is_empty() { &options.playlist } else { &info.playlist };
        if !usable_name(playlist) {
            bail!("{file_name}: playlist name is empty, too long, or has control characters");
        }
        // Fold smart quotes before the name becomes an asset path or a song id, so a mis-tagged
        // title cannot produce a byte-identical lowercased asset name.
        let (artist, title) = (sanitize_text(&info.artist), sanitize_text(&info.title));
        let slug = slug_of(&artist, &title);
        let song = Song { info, artist, title, slug: String::new(), playlist: playlist.clone() };
        if !ids.insert(song.id()) {
            bail!("Two songs are both \"{}\"", song.id());
        }
        // The bundle stores asset names lowercased, so uniqueness is case-insensitive too.
        songs.push(Song { slug: unique_slug(&slug, &mut slugs), ..song });
    }

    let mut assets: Vec<Built> = Vec::new();
    let mut resources: Vec<Built> = Vec::new();
    let mut chunks: Vec<TocChunk> = Vec::new();
    let mut chunk_data: Vec<Vec<u8>> = Vec::new();
    for (index, song) in songs.iter().enumerate() {
        check(cancel)?;
        report(index, Stage::Encoding, String::new());
        let encoded = audio::encode(&song.info.file, options.bitrate, options.normalize)?;
        let opus = &encoded.opus;
        let samples = playable_samples(opus);
        let header = codec_header(&template_header, samples)?;
        let stream = stream_chunk(opus, &header)?;
        let prefetch = prefetch_chunk(&template_prefetch, opus, &header, &stream)?;
        report(
            index,
            Stage::Encoded,
            format!(
                "{:.1} s, {} packets, {} KB{}",
                opus.samples as f64 / 48000.0,
                opus.packets.len(),
                stream.len() / 1024,
                if encoded.cached { " (cached)" } else { "" }
            ),
        );

        // The wave: a fresh partition with its own two chunks.
        let mut wave = ebx::read_document(&wave_bytes)?;
        renew(&mut wave);
        let wave_name = format!("{ASSET_FOLDER}{}_NWA", song.slug);
        let wave_instance = wave.instance_of("NewWaveAsset")?.instance_guid;
        let wave_root = wave.object_of("NewWaveAsset")?;
        *wave_root.field_mut("Name")? = text(&wave_name);
        let wave_chunks = array_mut(wave_root.field_mut("Chunks")?)?;
        if wave_chunks.len() != 2 {
            bail!("Template wave does not have two chunks");
        }
        for (entry, bytes) in wave_chunks.iter_mut().zip([&prefetch, &stream]) {
            let entry = object_mut(entry)?;
            let guid = Guid::random();
            *entry.field_mut("ChunkId")? = Value::Guid(guid);
            set_number(entry.field_mut("ChunkSize")?, bytes.len() as u64);
            chunks.push(TocChunk { guid, ..Default::default() });
            chunk_data.push(bytes.clone());
        }

        // The song: a fresh partition, its music segment's wave import pointing at the new wave.
        let mut graph = ebx::read_document(&song_bytes)?;
        renew(&mut graph);
        let mut rewired = false;
        for import in graph.imports.iter_mut().filter(|import| import.file_guid == template_wave_file) {
            import.file_guid = wave.file_guid;
            import.class_guid = wave_instance;
            rewired = true;
        }
        if !rewired {
            bail!("Template song does not import its wave");
        }
        let song_name = format!("{ASSET_FOLDER}{}_MG", song.slug);
        let graph_instance = graph.instance_of("MusicGraphAsset")?.instance_guid;
        let graph_root = graph.object_of("MusicGraphAsset")?;
        *graph_root.field_mut("Name")? = text(&song_name);
        set_number(graph_root.field_mut("NameHash")?, name_hash(&song_name) as u64);
        // No station tag: the template's would list the song under its station (Turkish). The
        // playlist comes from reskate-music.json instead; the licensed songs carry no tags either.
        // The orphaned MusicAssetTag instance is harmless.
        array_mut(graph_root.field_mut("TagRefs")?)?.clear();
        array_mut(graph_root.field_mut("Tags")?)?.clear();
        let metadata = graph.object_of("DingoMusicMetadata")?;
        *metadata.field_mut("ArtistName")? = text(&song.artist);
        *metadata.field_mut("TrackName")? = text(&song.title);

        playlist.imports.push(ebx::ImportReference { file_guid: graph.file_guid, class_guid: graph_instance });
        let import_index = playlist.imports.len() as i32 - 1;
        array_mut(playlist.object_of("MusicPlaylistAsset")?.field_mut("Assets")?)?
            .push(Value::Pointer(PointerReference { kind: PointerKind::External, index: import_index }));

        let facts = WaveFacts {
            instance: wave_instance,
            prefetch_id: chunks[chunks.len() - 2].guid,
            stream_id: chunks[chunks.len() - 1].guid,
            prefetch_size: prefetch.len() as u32,
            stream_size: stream.len() as u32,
            samples,
            name: wave_name.clone(),
        };
        assets.push(Built { name: wave_name.to_lowercase(), payload: ebx::write_document(&wave)? });
        assets.push(Built { name: song_name.to_lowercase(), payload: ebx::write_document(&graph)? });
        resources.push(Built {
            name: wave_name.to_lowercase(),
            payload: wave_resource(&template_blob, &template_facts, &facts)?,
        });
    }

    report(count, Stage::Building, String::new());
    // Bundle: the shipped manifest plus the new assets and wave resources, the master playlist
    // rewritten. Region files run parallel to the manifest: ebx, then resources, then chunks.
    let ebx_options = EncodeOptions::new(&options.game, CasCompression::Kraken);
    let compressed = oodle::can_compress(&options.game);
    let mut manifest = bundle.manifest.clone();
    let (ebx_count, resource_count) = (manifest.ebx.len(), manifest.resources.len());
    let shipped_files = &bundle.payloads;
    if shipped_files.len() < ebx_count + resource_count {
        bail!("The shipped bundle's payloads do not match its manifest");
    }
    let mut ebx_files = shipped_files[..ebx_count].to_vec();
    let mut resource_files = shipped_files[ebx_count..ebx_count + resource_count].to_vec();
    let chunk_files = shipped_files[ebx_count + resource_count..].to_vec();
    // (is a resource, index in its list, encoded payload)
    let mut placed: Vec<(bool, usize, Vec<u8>)> = Vec::new();

    let playlist_payload = ebx::write_document(&playlist)?;
    manifest.ebx[playlist_index].sha1 = Sha1::of(&playlist_payload);
    manifest.ebx[playlist_index].original_size = playlist_payload.len() as u64;
    placed.push((false, playlist_index, encode_cas(&playlist_payload, &ebx_options)?));
    for asset in &assets {
        let mut entry = manifest.ebx[song_index].clone();
        entry.name = asset.name.as_bytes().to_vec();
        entry.sha1 = Sha1::of(&asset.payload);
        entry.original_size = asset.payload.len() as u64;
        manifest.ebx.push(entry);
        ebx_files.push(BundleFileInfo::default());
        placed.push((false, ebx_files.len() - 1, encode_cas(&asset.payload, &ebx_options)?));
    }
    for resource in &resources {
        let mut entry = template_resource.clone();
        entry.name = resource.name.as_bytes().to_vec();
        entry.resource_id = u64::from_le_bytes(Guid::random().0[..8].try_into().expect("8 bytes"));
        entry.sha1 = Sha1::of(&resource.payload);
        entry.original_size = resource.payload.len() as u64;
        manifest.resources.push(entry);
        resource_files.push(BundleFileInfo::default());
        placed.push((true, resource_files.len() - 1, encode_cas(&resource.payload, &ebx_options)?));
    }
    let manifest_bytes = write_binary_bundle(&manifest)?;
    let mut cas = manifest_bytes.clone();
    for (is_resource, index, encoded) in &placed {
        let list = if *is_resource { &mut resource_files } else { &mut ebx_files };
        list[*index] = BundleFileInfo { location: MOD_ARCHIVE, offset: cas.len() as u32, size: encoded.len() as u32 };
        cas.extend_from_slice(encoded);
    }
    let mut region_files = vec![BundleFileInfo { location: MOD_ARCHIVE, offset: 0, size: manifest_bytes.len() as u32 }];
    region_files.extend(ebx_files.iter().chain(&resource_files).chain(&chunk_files).copied());
    if region_files.len() != 1 + manifest.ebx.len() + manifest.resources.len() + manifest.chunks.len() {
        bail!("Placement count does not match the manifest");
    }

    // Audio chunks: stored uncompressed (Opus does not compress), referenced from the TOC.
    let raw = EncodeOptions::new(&options.game, CasCompression::Raw);
    let mut audio_bytes = 0;
    for (chunk, bytes) in chunks.iter_mut().zip(&chunk_data) {
        let encoded = encode_cas(bytes, &raw)?;
        audio_bytes += bytes.len();
        *chunk = TocChunk { location: MOD_ARCHIVE, offset: cas.len() as u32, size: encoded.len() as u32, ..*chunk };
        cas.extend_from_slice(&encoded);
        if cas.len() > u32::MAX as usize {
            bail!("Too much audio for one archive");
        }
    }

    let patched = TocBundle {
        name: BUNDLE_NAME.into(),
        region: write_bundle_region(&region_files, &[])?,
        load_flag: shipped.load_flag,
    };
    let toc_bytes = write_patch_toc(&[patched], &chunks, 3)?;
    verify_toc(&read_toc(&toc_bytes, &[])?)?;

    report(count, Stage::Writing, String::new());
    write_file(&out.join(CAS_RELATIVE), &cas)?;
    write_file(&out.join(TOC_PATH), &toc_bytes)?;
    let layout = options.game.join("Data").join("layout.toc");
    std::fs::copy(&layout, out.join("layout.toc")).at("Cannot copy", &layout)?;
    let skate_sha256 = tools::sha256_hex_of_file(&options.game.join("Skate.exe"))?;
    write_file(
        &out.join(".reskate-studio-patch"),
        format!("ReSkate Studio native Patch v1\nskate_sha256={skate_sha256}\n").as_bytes(),
    )?;
    let manifest_json = format!(
        "{{\n  \"name\": {},\n  \"version_number\": \"1.0.0\",\n  \"description\": \"Adds {count} song(s) to the game's music.\",\n  \"dependencies\": []\n}}\n",
        Json::String(options.name.clone())
    );
    write_file(&out.join("manifest.json"), manifest_json.as_bytes())?;
    write_music_json(options, out, files, &songs, cancel)?;
    write_file(&out.join("reskate-build.json"), BUILD_MARKER.as_bytes())?;
    // What the mod was made from, so it can be reopened and changed (load_project).
    write_file(&out.join("reskate-music-project.json"), project::to_json(options, files).as_bytes())?;
    Ok(PackResult { songs: count, audio_kb: audio_bytes / 1024, output: out.to_path_buf(), compressed })
}

/// reskate-music.json: the playlists ReSkate's music menu shows (songs by their "artist - title"
/// ids, in file order) and their covers, which are written to artwork/.
fn write_music_json(
    options: &PackOptions,
    out: &Path,
    files: &[SongInfo],
    songs: &[Song<'_>],
    cancel: &AtomicBool,
) -> Result<()> {
    let mut playlist_order: Vec<&str> = Vec::new();
    let mut playlist_songs: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for song in songs {
        if !playlist_songs.contains_key(song.playlist.as_str()) {
            playlist_order.push(&song.playlist);
        }
        playlist_songs.entry(&song.playlist).or_default().push(song.id());
    }

    // Each distinct source image is converted and packaged once.
    let mut images: HashMap<PathBuf, String> = HashMap::new();
    let mut image = |source: &Path| -> Result<String> {
        check(cancel)?;
        let absolute = std::path::absolute(source).unwrap_or_else(|_| source.to_path_buf());
        if let Some(relative) = images.get(&absolute) {
            return Ok(relative.clone());
        }
        if !absolute.is_file() {
            bail!("Artwork file is missing: {}", absolute.display());
        }
        let relative = format!("artwork/cover-{}.png", images.len());
        write_file(&out.join(&relative), &image_artwork_png(&absolute)?)?;
        check(cancel)?;
        images.insert(absolute, relative.clone());
        Ok(relative)
    };

    // Track artwork is resolved first so a playlist can borrow its first available cover.
    let mut covers = Map::new();
    for (info, song) in files.iter().zip(songs) {
        check(cancel)?;
        let source = info.artwork.clone().or_else(|| embedded_artwork(&info.file));
        if let Some(source) = source {
            covers.insert(song.id(), Json::String(image(&source)?));
        }
    }
    let mut playlists = Vec::new();
    for (index, name) in playlist_order.iter().enumerate() {
        let list = &playlist_songs[name];
        let mut entry = Map::new();
        entry.insert("name".into(), json!(name));
        entry.insert("songs".into(), json!(list));
        if let Some(path) = options.playlist_artwork.get(*name).filter(|path| !path.as_os_str().is_empty()) {
            entry.insert("artwork".into(), Json::String(image(path)?));
        } else if options.generated_playlist_artwork.contains(*name) {
            check(cancel)?;
            let relative = format!("artwork/playlist-{index}.png");
            write_file(&out.join(&relative), &playlist_artwork_png(name)?)?;
            entry.insert("artwork".into(), Json::String(relative));
        } else if let Some(cover) = list.iter().find_map(|id| covers.get(id)) {
            entry.insert("artwork".into(), cover.clone());
        }
        playlists.push(Json::Object(entry));
    }
    let mut metadata = Map::new();
    metadata.insert("schema".into(), json!(1));
    metadata.insert("playlists".into(), Json::Array(playlists));
    if !covers.is_empty() {
        metadata.insert("song_artwork".into(), Json::Object(covers));
    }
    write_file(&out.join("reskate-music.json"), project::pretty(Json::Object(metadata)).as_bytes())
}
