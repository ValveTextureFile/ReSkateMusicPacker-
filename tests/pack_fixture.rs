// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The packer end to end against a synthetic game: a layout.toc, one cas archive and a
//! bam_levelroot TOC holding minimal template assets shaped like the shipped ones. Two generated
//! tones go in; the mod that comes out is read back with the engine. Needs ffmpeg and ffprobe
//! (skips without them).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use reskate_music::engine::binary::{Guid, LE, Sha1, Writer};
use reskate_music::engine::bundle::*;
use reskate_music::engine::cas::{CasCompression, DecodeOptions, EncodeOptions, decode_cas, encode_cas};
use reskate_music::engine::ebx::{self, *};
use reskate_music::engine::native_db::{ENVELOPE_SIZE, MAGIC};
use reskate_music::engine::toc::*;
use reskate_music::music::wave::{WAVE_RESOURCE_TYPE, djb2};
use reskate_music::music::{self, PackOptions, SongInfo, ThunderstoreOptions};
use reskate_music::{Error, music::tools};

const INSTALL_CHUNK: u32 = 2_016_875_820;
const PACKAGE_DIR: &str = "configurations/layout/defaultinstallpackage";
const BUNDLE: &str = "win32/levels/game/bam_levelroot/bam_coregameassets";
const SONG: &str = "audio/music/assets/turkish/turkish_upm_belly_dancer_mg";
const WAVE: &str = "audio/music/assets/turkish/turkish_upm_belly_dancer_chapwr13a_1_nwa";
const PLAYLIST: &str = "audio/music/playlist/dgo_mus_playlist";

// ---- reflection ----------------------------------------------------------------------------------

struct FieldSpec(&'static str, u8, u8, u32, u16); // name, kind, category, offset, class
struct TypeSpec(&'static str, u16, &'static [FieldSpec]); // name, size, fields

const CSTRING: (u8, u8) = (kind::CSTRING, 3);
const TYPES: &[TypeSpec] = &[
    TypeSpec(
        "NewWaveAsset",
        32,
        &[
            FieldSpec("Name", CSTRING.0, CSTRING.1, 8, 0),
            FieldSpec("Chunks", kind::STRUCTURE, kind::CATEGORY_ARRAY, 16, 1),
        ],
    ),
    TypeSpec(
        "WaveChunk",
        24,
        &[FieldSpec("ChunkId", kind::GUID, 3, 0, 0), FieldSpec("ChunkSize", kind::UINT32, 3, 16, 0)],
    ),
    TypeSpec(
        "MusicGraphAsset",
        48,
        &[
            FieldSpec("Name", CSTRING.0, CSTRING.1, 8, 0),
            FieldSpec("NameHash", kind::UINT32, 3, 16, 0),
            FieldSpec("TagRefs", kind::POINTER, kind::CATEGORY_ARRAY, 24, 0),
            FieldSpec("Tags", kind::POINTER, kind::CATEGORY_ARRAY, 32, 0),
            FieldSpec("Segment", kind::POINTER, 1, 40, 0),
        ],
    ),
    TypeSpec(
        "DingoMusicMetadata",
        24,
        &[FieldSpec("ArtistName", CSTRING.0, CSTRING.1, 8, 0), FieldSpec("TrackName", CSTRING.0, CSTRING.1, 16, 0)],
    ),
    TypeSpec("MusicPlaylistAsset", 16, &[FieldSpec("Assets", kind::POINTER, kind::CATEGORY_ARRAY, 8, 0)]),
];

fn type_guid(index: usize) -> Guid {
    Guid([0x40 + index as u8; 16])
}

/// A document shell with the shared reflection table, its type and field descriptors filled in.
fn shell(fixup_types: &[usize]) -> Document {
    let mut strings = Vec::new();
    let mut name = |text: &str| {
        let offset = strings.len() as u32;
        strings.extend_from_slice(text.as_bytes());
        strings.push(0);
        offset
    };
    let mut document = Document::default();
    let mut w = Writer::new();
    w.i32(TYPES.len() as i32, LE);
    for index in 0..TYPES.len() {
        w.guid(&type_guid(index), LE);
        w.u32(0x100 + index as u32, LE);
    }
    w.i32(TYPES.len() as i32, LE);
    let mut first = 0;
    for (index, ty) in TYPES.iter().enumerate() {
        let hash = name(ty.0);
        w.u32(hash, LE);
        w.i32(first, LE);
        w.u16(ty.2.len() as u16, LE);
        w.u16(0x22 << 1, LE);
        w.u16(ty.1, LE);
        w.u16(8, LE);
        document.types.push(TypeDescriptor {
            name: ty.0.into(),
            name_hash: hash,
            field_index: first,
            field_count: ty.2.len() as u16,
            encoded_type: 0x22,
            size: ty.1,
            alignment: 8,
            guid: type_guid(index),
            signature: 0x100 + index as u32,
        });
        first += ty.2.len() as i32;
    }
    w.i32(first, LE);
    for ty in TYPES {
        for field in ty.2 {
            let hash = name(field.0);
            let encoded = ((field.1 as u16) << 4) | field.2 as u16;
            w.u32(hash, LE);
            w.u32(field.3, LE);
            w.u16(encoded << 1, LE);
            w.u16(field.4, LE);
            document.fields.push(FieldDescriptor {
                name: field.0.into(),
                name_hash: hash,
                encoded_type: encoded,
                class_ref: field.4,
                data_offset: field.3,
            });
        }
    }
    w.i32(0, LE);
    w.i32(0, LE);
    w.bytes(&strings);
    document.reflection_data = w.into_inner();
    document.fixup_type_guids = fixup_types.iter().map(|&index| type_guid(index)).collect();
    document.fixup_type_signatures = fixup_types.iter().map(|&index| 0x100 + index as u32).collect();
    document
}

fn object(document: &Document, type_index: usize, values: Vec<Value>) -> Object {
    let first = document.types[type_index].field_index as usize;
    let fields = values
        .into_iter()
        .enumerate()
        .map(|(ordinal, value)| FieldValue {
            descriptor: first + ordinal,
            name: document.fields[first + ordinal].name.clone(),
            value,
        })
        .collect();
    Object { descriptor: type_index as i32, fields }
}

fn instance(fixup_type: u16, descriptor: usize, exported: bool, object: Object) -> InstanceRecord {
    InstanceRecord {
        fixup_type,
        descriptor: descriptor as i32,
        exported,
        instance_guid: if exported { Guid::random() } else { Guid::default() },
        object: Some(object),
        ..Default::default()
    }
}

fn bytes(text: &str) -> Value {
    Value::Bytes(text.as_bytes().to_vec())
}

fn null() -> Value {
    Value::Pointer(PointerReference::default())
}

// ---- layout.toc ----------------------------------------------------------------------------------

fn varint(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = (value & 127) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 128);
    }
}

enum Db {
    Object(Vec<(&'static str, Db)>),
    List(Vec<Db>),
    Text(String),
    Int(u32),
}

fn db(name: Option<&str>, node: &Db, out: &mut Vec<u8>) {
    let kind = match node {
        Db::Object(_) => 2,
        Db::List(_) => 1,
        Db::Text(_) => 7,
        Db::Int(_) => 8,
    };
    out.push(kind | if name.is_some() { 0 } else { 0x80 });
    if let Some(name) = name {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    match node {
        Db::Object(fields) => {
            let mut body = Vec::new();
            for (name, child) in fields {
                db(Some(name), child, &mut body);
            }
            body.push(0);
            varint(out, body.len());
            out.extend_from_slice(&body);
        }
        Db::List(items) => {
            let mut body = Vec::new();
            for item in items {
                db(None, item, &mut body);
            }
            body.push(0);
            varint(out, body.len());
            out.extend_from_slice(&body);
        }
        Db::Text(text) => {
            varint(out, text.len() + 1);
            out.extend_from_slice(text.as_bytes());
            out.push(0);
        }
        Db::Int(value) => out.extend_from_slice(&value.to_le_bytes()),
    }
}

fn layout_toc() -> Vec<u8> {
    let root = Db::Object(vec![(
        "installManifest",
        Db::Object(vec![(
            "installChunks",
            Db::List(vec![Db::Object(vec![
                ("name", Db::Text(PACKAGE_DIR.into())),
                ("persistentIndex", Db::Int(INSTALL_CHUNK)),
            ])]),
        )]),
    )]);
    let mut out = vec![0u8; ENVELOPE_SIZE];
    out[..4].copy_from_slice(&MAGIC);
    db(None, &root, &mut out);
    out
}

// ---- the game ------------------------------------------------------------------------------------

struct Fixture {
    game: PathBuf,
    template_wave_file: Guid,
    template_wave_instance: Guid,
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn make_game(root: &Path) -> Fixture {
    let game = root.join("game");
    write(&game.join("Skate.exe"), b"MZ not really a game");
    write(&game.join("Data/layout.toc"), &layout_toc());

    // The template wave: two chunks, a 2-second prefetch-declared length.
    let prefetch_id = Guid::random();
    let stream_id = Guid::random();
    let mut header = [0x12, 0x34, 0x56, 0x78, 0, 0, 0, 0x05, 0x9A, 0xBC];
    header[4..8].copy_from_slice(&((96_000u32 << 6) | 0x05).to_be_bytes());
    let mut prefetch = vec![1, 0x10, 0x01, 0x38, 0, 0, 0, 22, 0, 0, 0, 0];
    prefetch.extend_from_slice(&header);
    prefetch.extend_from_slice(&[0x01, 0x02]);
    let stream_size = 1000u32;

    let mut wave = shell(&[0, 1]);
    let chunk = |id: Guid, size: u32| {
        Value::Object(Box::new(object(&wave, 1, vec![Value::Guid(id), Value::UInt(size as u64)])))
    };
    let chunks = vec![chunk(prefetch_id, prefetch.len() as u32), chunk(stream_id, stream_size)];
    let wave_object = object(&wave, 0, vec![bytes("Audio/Music/Assets/Turkish/Belly_NWA"), Value::Array(chunks)]);
    wave.file_guid = Guid::random();
    wave.instances = vec![instance(0, 0, true, wave_object)];
    let wave_instance = wave.instances[0].instance_guid;

    let mut song = shell(&[2, 3]);
    song.file_guid = Guid::random();
    song.imports = vec![ImportReference { file_guid: wave.file_guid, class_guid: wave_instance }];
    let segment = Value::Pointer(PointerReference { kind: PointerKind::External, index: 0 });
    let graph = object(
        &song,
        2,
        vec![
            bytes("Audio/Music/Assets/Turkish/Belly_MG"),
            Value::UInt(1234),
            Value::Array(vec![null()]),
            Value::Array(vec![null()]),
            segment,
        ],
    );
    let metadata = object(&song, 3, vec![bytes("Turkish Artist"), bytes("Belly Dancer")]);
    song.instances = vec![instance(0, 2, true, graph), instance(1, 3, false, metadata)];

    let mut playlist = shell(&[4]);
    playlist.file_guid = Guid::random();
    playlist.imports = vec![ImportReference { file_guid: song.file_guid, class_guid: song.instances[0].instance_guid }];
    let assets = vec![Value::Pointer(PointerReference { kind: PointerKind::External, index: 0 })];
    playlist.instances = vec![instance(0, 4, true, object(&playlist, 4, vec![Value::Array(assets)]))];

    // The sound bank: every value the packer replaces, the expected number of times, over filler
    // that cannot match any of them.
    let mut blob = vec![0xEEu8; 240];
    let wave_id = djb2(wave_instance.to_string().as_bytes()).to_le_bytes();
    for k in 0..7 {
        blob[k * 4..k * 4 + 4].copy_from_slice(&wave_id);
    }
    blob[32..36].copy_from_slice(&(2.0f32).to_le_bytes());
    blob[36..40].copy_from_slice(&(prefetch.len() as u32).to_le_bytes());
    blob[40..44].copy_from_slice(&(stream_size - prefetch.len() as u32).to_le_bytes());
    blob[44..48].copy_from_slice(&(stream_size - prefetch.len() as u32).to_le_bytes());
    blob[48..64].copy_from_slice(&prefetch_id.0);
    blob[64..80].copy_from_slice(&stream_id.0);
    blob[80..96].copy_from_slice(&wave_instance.0);
    blob[216..220].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    blob[220..224].copy_from_slice(&0x1234_5678u32.to_le_bytes());

    // The archive: the manifest first (raw, like the game's), then each payload.
    let raw = EncodeOptions::new(&game, CasCompression::Raw);
    let location = CasIdentifier { patch: false, install_chunk: INSTALL_CHUNK, archive: 1 };
    let payloads: Vec<Vec<u8>> =
        [&wave, &song, &playlist].iter().map(|doc| ebx::write_document(doc).unwrap()).collect();
    let asset = |name: &str, payload: &[u8]| BundleAsset {
        name: name.as_bytes().to_vec(),
        sha1: Sha1::of(payload),
        original_size: payload.len() as u64,
        ..Default::default()
    };
    let manifest = BinaryBundle {
        ebx: vec![asset(WAVE, &payloads[0]), asset(SONG, &payloads[1]), asset(PLAYLIST, &payloads[2])],
        resources: vec![BundleAsset {
            kind: AssetKind::Resource,
            resource_type: WAVE_RESOURCE_TYPE,
            resource_meta: vec![0; 16],
            resource_id: 77,
            ..asset(WAVE, &blob)
        }],
        ..Default::default()
    };
    let manifest_bytes = write_binary_bundle(&manifest).unwrap();
    let mut cas = manifest_bytes.clone();
    let mut files = vec![BundleFileInfo { location, offset: 0, size: manifest_bytes.len() as u32 }];
    for payload in payloads.iter().chain([&blob]) {
        let encoded = encode_cas(payload, &raw).unwrap();
        files.push(BundleFileInfo { location, offset: cas.len() as u32, size: encoded.len() as u32 });
        cas.extend_from_slice(&encoded);
    }
    let prefetch_encoded = encode_cas(&prefetch, &raw).unwrap();
    let chunk = TocChunk {
        guid: prefetch_id,
        location,
        offset: cas.len() as u32,
        size: prefetch_encoded.len() as u32,
        removed: false,
    };
    cas.extend_from_slice(&prefetch_encoded);
    write(&game.join("Data/Win32").join(PACKAGE_DIR).join("cas_01.cas"), &cas);

    let region = write_bundle_region(&files, &[]).unwrap();
    let toc = write_patch_toc(&[TocBundle { name: BUNDLE.into(), region, load_flag: 1 }], &[chunk], 3).unwrap();
    write(&game.join("Data/Win32/levels/game/bam_levelroot/bam_levelroot.toc"), &toc);
    Fixture { game, template_wave_file: wave.file_guid, template_wave_instance: wave_instance }
}

fn tone(path: &Path, hertz: u32, artist: &str, title: &str) {
    let mut command = tools::command("ffmpeg");
    command
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", &format!("sine=frequency={hertz}:duration=3")])
        .args(["-metadata", &format!("artist={artist}"), "-metadata", &format!("title={title}")])
        .arg(path);
    tools::run(command).unwrap();
}

/// Reads one asset of the built mod back out of its archive.
fn read_payload(cas: &[u8], file: &BundleFileInfo) -> Vec<u8> {
    let raw = &cas[file.offset as usize..(file.offset + file.size) as usize];
    decode_cas(raw, &DecodeOptions::new(Path::new(""))).unwrap()
}

#[test]
fn packs_songs_into_a_mod_the_engine_can_read_back() {
    if !tools::locate(None).0 {
        eprintln!("SKIP: ffmpeg and ffprobe are not on PATH");
        return;
    }
    let root = std::env::temp_dir().join(format!("reskate-pack-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let fixture = make_game(&root);
    std::fs::create_dir_all(root.join("songs")).unwrap();
    let files = vec![root.join("songs/a.mp3"), root.join("songs/b.flac")];
    tone(&files[0], 440, "Test Artist", "Tone A");
    tone(&files[1], 880, "Test Artist", "Tone B");

    let mut songs = music::scan(&files);
    assert_eq!((songs[0].artist.as_str(), songs[0].title.as_str()), ("Test Artist", "Tone A"));
    assert!(songs[0].seconds > 2.9 && songs[0].seconds < 3.2, "length {}", songs[0].seconds);
    assert!(songs[0].problems.is_empty(), "{:?}", songs[0].problems);
    songs[1].playlist = "Second Station".into();

    let options = PackOptions {
        game: fixture.game.clone(),
        output: root.join("mod"),
        name: "Tones Mod".into(),
        playlist: "Tones".into(),
        generated_playlist_artwork: ["Second Station".to_string()].into(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let mut stages = Vec::new();
    let result = music::pack(&options, &songs, &mut |step| stages.push((step.song, step.stage)), &cancel).unwrap();
    assert_eq!(result.songs, 2);
    assert!(stages.contains(&(2, music::Stage::Writing)));
    let out = &options.output;
    for file in [
        "layout.toc",
        "manifest.json",
        "reskate-music.json",
        "reskate-build.json",
        "reskate-music-project.json",
        ".reskate-studio-patch",
    ] {
        assert!(out.join(file).is_file(), "{file} is missing");
    }
    assert!(!root.join("mod.partial").exists());

    // The TOC resolves and names one bundle whose manifest is the archive's first file.
    let toc =
        read_toc(&std::fs::read(out.join("Win32/levels/game/bam_levelroot/bam_levelroot.toc")).unwrap(), &[]).unwrap();
    verify_toc(&toc).unwrap();
    assert_eq!(toc.bundles.len(), 1);
    assert_eq!(toc.chunks.len(), 4);
    let cas = std::fs::read(out.join("Win32/configurations/layout/defaultinstallpackage/cas_01.cas")).unwrap();
    let region = read_bundle_region(&toc.bundles[0].region).unwrap();
    let manifest = read_binary_bundle(&cas[..region.files[0].size as usize]).unwrap();
    assert_eq!(manifest.ebx.len(), 3 + 4);
    assert_eq!(manifest.resources.len(), 1 + 2);
    assert_eq!(region.files.len(), 1 + 7 + 3);
    let shipped: Vec<bool> = region.files.iter().map(|file| !file.location.patch).collect();
    assert_eq!(shipped, [false, true, true, false, false, false, false, false, true, false, false]);
    let names: Vec<String> =
        manifest.ebx.iter().map(|asset| String::from_utf8_lossy(&asset.name).into_owned()).collect();
    assert!(names.contains(&"audio/music/assets/reskate/test_artist_tone_a_mg".to_string()), "{names:?}");
    assert!(names.contains(&"audio/music/assets/reskate/test_artist_tone_b_nwa".to_string()));
    // Untouched shipped assets still point into the game's own archive; the rest are the mod's.
    for (asset, file) in manifest.ebx.iter().zip(&region.files[1..]).filter(|(_, file)| file.location.patch) {
        let payload = read_payload(&cas, file);
        assert_eq!(Sha1::of(&payload), asset.sha1);
        assert_eq!(payload.len() as u64, asset.original_size);
    }

    // The playlist lists the template song and both new songs.
    let playlist_index = names.iter().position(|name| name == PLAYLIST).unwrap();
    let playlist = ebx::read_document(&read_payload(&cas, &region.files[1 + playlist_index])).unwrap();
    let Value::Array(assets) = playlist.instances[0].object.as_ref().unwrap().field("Assets").unwrap() else {
        panic!()
    };
    assert_eq!(assets.len(), 3);
    assert_eq!(playlist.imports.len(), 3);

    // Song A: renamed, re-tagged, and wired to its own fresh wave.
    let song_index = names.iter().position(|name| name.ends_with("tone_a_mg")).unwrap();
    let wave_index = names.iter().position(|name| name.ends_with("tone_a_nwa")).unwrap();
    let graph = ebx::read_document(&read_payload(&cas, &region.files[1 + song_index])).unwrap();
    let wave = ebx::read_document(&read_payload(&cas, &region.files[1 + wave_index])).unwrap();
    assert_ne!(wave.file_guid, fixture.template_wave_file);
    assert_ne!(wave.instances[0].instance_guid, fixture.template_wave_instance);
    assert_eq!(
        graph.imports[0],
        ImportReference { file_guid: wave.file_guid, class_guid: wave.instances[0].instance_guid }
    );
    assert_eq!(playlist.imports[1].file_guid, graph.file_guid);
    let root_object = graph.instances[0].object.as_ref().unwrap();
    assert_eq!(root_object.field("Name").unwrap(), &bytes("Audio/Music/Assets/ReSkate/Test_Artist_Tone_A_MG"));
    assert_eq!(root_object.field("NameHash").unwrap(), &Value::UInt(djb2(b"Test_Artist_Tone_A_MG") as u64));
    assert_eq!(root_object.field("Tags").unwrap(), &Value::Array(vec![]));
    let metadata = graph.instances[1].object.as_ref().unwrap();
    assert_eq!(metadata.field("TrackName").unwrap(), &bytes("Tone A"));

    // Its audio chunks are where the wave says, the prefetch chunk carries the stream's sample count.
    let Value::Array(chunks) = wave.instances[0].object.as_ref().unwrap().field("Chunks").unwrap() else { panic!() };
    let chunk_of = |value: &Value| {
        let Value::Object(entry) = value else { panic!() };
        let Value::Guid(id) = entry.field("ChunkId").unwrap() else { panic!() };
        let Value::UInt(size) = entry.field("ChunkSize").unwrap() else { panic!() };
        let chunk = toc.chunks.iter().find(|chunk| chunk.guid == *id).expect("chunk in TOC");
        let data =
            read_payload(&cas, &BundleFileInfo { location: chunk.location, offset: chunk.offset, size: chunk.size });
        assert_eq!(data.len() as u64, *size);
        data
    };
    let prefetch = chunk_of(&chunks[0]);
    let stream = chunk_of(&chunks[1]);
    assert_eq!(&prefetch[..2], &[1, 0x10]);
    assert_eq!(&prefetch[12..16], &[0x12, 0x34, 0x56, 0x78]);
    let header: [u8; 10] = prefetch[12..22].try_into().unwrap();
    let samples = music::audio::header_samples(&header);
    assert!((samples as f64 / 48000.0 - 3.0).abs() < 0.05, "{samples} samples");
    assert_eq!(stream[0], b'H');
    assert_eq!(&stream[stream.len() - 5..], &[b'E', 0, 0, 5, 0]);
    let entries = music::audio::seek_entries(&stream).unwrap();
    assert_eq!(entries.iter().map(|entry| entry[2]).sum::<i64>() as u64, samples);

    // reskate-music.json: both playlists, the second with its generated cover.
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("reskate-music.json")).unwrap()).unwrap();
    assert_eq!(json["playlists"][0]["name"], "Tones");
    assert_eq!(json["playlists"][0]["songs"][0], "Test Artist - Tone A");
    assert_eq!(json["playlists"][1]["artwork"], "artwork/playlist-1.png");
    assert!(out.join("artwork/playlist-1.png").is_file());
    let project = music::load_project(out).unwrap();
    assert_eq!(project.name, "Tones Mod");
    assert_eq!(project.songs[1].playlist, "Second Station");

    // Rebuilding reuses the cached encodes.
    let mut cached = 0;
    let rebuild = PackOptions { output: root.join("mod2"), ..options.clone() };
    music::pack(&rebuild, &songs, &mut |step| cached += step.detail.contains("(cached)") as usize, &cancel).unwrap();
    assert_eq!(cached, 2);

    // A Thunderstore package of the mod.
    let zip =
        music::export_thunderstore(out, &ThunderstoreOptions { author: "Me".into(), ..Default::default() }).unwrap();
    assert_eq!(zip.file_name().unwrap(), "Me-Tones_Mod-1.0.0.zip");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
    let names: Vec<String> = archive.file_names().map(|name| name.unwrap().into_owned()).collect();
    assert!(names.contains(&"icon.png".to_string()) && names.contains(&"README.md".to_string()));
    assert!(names.contains(&"Win32/configurations/layout/defaultinstallpackage/cas_01.cas".to_string()));
    assert!(!names.iter().any(|name| name.contains("project") || name.contains('\\')));
    let readme = std::io::read_to_string(archive.by_name("README.md").unwrap()).unwrap();
    assert!(readme.contains("- Test Artist - Tone B") && readme.contains("Packaged with"));

    // Refusals leave nothing behind.
    let cancelled = AtomicBool::new(true);
    let target = PackOptions { output: root.join("cancelled"), ..options.clone() };
    assert!(matches!(music::pack(&target, &songs, &mut |_| {}, &cancelled), Err(Error::Cancelled)));
    assert!(!root.join("cancelled").exists() && !root.join("cancelled.partial").exists());
    let twice = vec![songs[0].clone(), SongInfo { file: files[1].clone(), ..songs[0].clone() }];
    let error = music::pack(&target, &twice, &mut |_| {}, &cancel).unwrap_err();
    assert!(error.to_string().contains("Two songs are both"), "{error}");
    let mut missing = songs.clone();
    missing[0].artwork = Some(root.join("missing.png"));
    let error = music::pack(&target, &missing, &mut |_| {}, &cancel).unwrap_err();
    assert!(error.to_string().contains("Artwork file is missing"), "{error}");
    assert!(!root.join("cancelled").exists() && !root.join("cancelled.partial").exists());

    let _ = std::fs::remove_dir_all(&root);
}
