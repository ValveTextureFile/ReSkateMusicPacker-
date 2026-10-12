// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The command line: pack a folder of songs, optionally export a Thunderstore package, or fetch
//! ffmpeg. Run with no arguments (or `--gui`) for the window.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use clap::Parser;
use reskate_music::music::scan::is_image;
use reskate_music::music::{self, PackOptions, Stage, ThunderstoreOptions, ffmpeg_fetch, tools};
use reskate_music::settings::Settings;
use reskate_music::{Error, Result};

#[derive(Parser, Debug)]
#[command(
    name = "ReSkateMusicPacker",
    about = "Packs a folder of songs into an add-only music mod for skate. (ReSkate).",
    after_help = "Run with no arguments (or --gui) for the graphical interface.",
    disable_version_flag = true
)]
struct Args {
    /// The ReSkate install containing Skate.exe.
    #[arg(value_name = "GAME FOLDER", required_unless_present = "get_ffmpeg")]
    game: Option<PathBuf>,
    /// A folder of audio files (read directly, not recursively; images are skipped).
    #[arg(value_name = "SONG FOLDER", required_unless_present = "get_ffmpeg")]
    songs: Option<PathBuf>,
    /// Where the mod is written [default: <song folder>_mod beside the song folder].
    #[arg(value_name = "OUTPUT FOLDER")]
    output: Option<PathBuf>,

    /// Display name of the mod [default: the song folder's name].
    #[arg(long, value_name = "MOD NAME")]
    name: Option<String>,
    /// Playlist name shown in the in-game music menu [default: the song folder's name].
    #[arg(long, value_name = "PLAYLIST")]
    playlist: Option<String>,
    /// Opus bitrate in kbps.
    #[arg(long, value_name = "KBPS", default_value_t = 192, value_parser = clap::value_parser!(u32).range(64..=320))]
    bitrate: u32,
    /// Disable EBU R128 loudness normalisation (two-pass to -15 LUFS).
    #[arg(long)]
    no_normalize: bool,
    /// Cover image for the named playlist.
    #[arg(long, num_args = 2, value_names = ["NAME", "IMAGE"], action = clap::ArgAction::Append)]
    playlist_artwork: Vec<OsString>,
    /// Cover for the exact "Artist - Title" song id; repeat for several.
    #[arg(long, num_args = 2, value_names = ["ID", "IMAGE"], action = clap::ArgAction::Append)]
    track_artwork: Vec<OsString>,
    /// Generate a text cover from the playlist name.
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    generate_playlist_artwork: Vec<String>,

    /// Also export a Thunderstore-ready .zip beside the mod.
    #[arg(long)]
    thunderstore: bool,
    /// Author namespace in the Thunderstore manifest.
    #[arg(long, value_name = "NAME", default_value = "Author")]
    author: String,
    /// Thunderstore package version.
    #[arg(long, value_name = "X.Y.Z", default_value = "1.0.0")]
    version: String,
    /// Omit the packer credit link from the generated README.
    #[arg(long)]
    no_readme_credit: bool,

    /// The folder holding ffmpeg and ffprobe [default: the window's setting, then PATH].
    #[arg(long, value_name = "FOLDER")]
    ffmpeg: Option<PathBuf>,
    /// Download the pinned static ffmpeg build into FOLDER (default: beside this program or the
    /// local app data folder), print where it went, then exit.
    #[arg(long, value_name = "FOLDER", num_args = 0..=1)]
    get_ffmpeg: Option<Option<PathBuf>>,
    /// Launch the graphical interface (with any songs or folders given added to the list).
    #[arg(long)]
    gui: bool,
}

fn pairs(values: &[OsString]) -> Vec<(String, PathBuf)> {
    values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0].to_string_lossy().into_owned(), PathBuf::from(&pair[1])))
        .collect()
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn get_ffmpeg(folder: Option<PathBuf>) -> Result<()> {
    let Some((url, sha)) = ffmpeg_fetch::source() else {
        return Err(Error::msg(
            "There is no pinned ffmpeg build for this system; install ffmpeg with your package manager \
             (for example `brew install ffmpeg`), or set FFMPEG_URL and FFMPEG_SHA256.",
        ));
    };
    let directory = match folder {
        Some(folder) => folder,
        None => ffmpeg_fetch::default_install_dir()?,
    };
    println!("Downloading ffmpeg into {} ...", directory.display());
    let cancel = AtomicBool::new(false);
    let mut progress = |step: ffmpeg_fetch::DownloadProgress| {
        if step.total != 0 {
            print!("\r  {} / {} MB", step.received >> 20, step.total >> 20);
        } else {
            print!("\r  {} MB", step.received >> 20);
        }
        let _ = std::io::stdout().flush();
    };
    ffmpeg_fetch::ensure_ffmpeg(&directory, &url, &sha, &mut progress, &cancel)?;
    println!("\nffmpeg -> {}", directory.display());
    Ok(())
}

fn pack(args: Args) -> Result<()> {
    let (Some(game), Some(song_folder)) = (args.game, args.songs) else { unreachable!("clap requires both") };
    if !reskate_music::settings::is_game_folder(&game) {
        return Err(Error::msg(format!("{} has no Skate.exe; give the ReSkate install folder", game.display())));
    }
    let folder_name = song_folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let playlist = args.playlist.unwrap_or_else(|| folder_name.clone());
    if !music::usable_name(&playlist) {
        return Err(Error::msg("the playlist name is empty, too long, or has control characters"));
    }
    let output = args.output.unwrap_or_else(|| {
        let mut name = song_folder.file_name().unwrap_or_default().to_os_string();
        name.push("_mod");
        song_folder.with_file_name(name)
    });
    let playlist_artwork: BTreeMap<String, PathBuf> = pairs(&args.playlist_artwork).into_iter().collect();
    let track_artwork = pairs(&args.track_artwork);
    let options = PackOptions {
        game,
        output,
        name: args.name.unwrap_or(folder_name),
        playlist,
        bitrate: args.bitrate,
        normalize: !args.no_normalize,
        playlist_artwork,
        generated_playlist_artwork: args.generate_playlist_artwork.into_iter().collect(),
    };

    let (found, _) = tools::locate(Some(args.ffmpeg.unwrap_or_else(|| Settings::load().ffmpeg)).as_deref());
    if !found {
        return Err(Error::msg(
            "ffmpeg and ffprobe were not found; install ffmpeg, pass --ffmpeg <folder>, or run --get-ffmpeg",
        ));
    }

    let entries = std::fs::read_dir(&song_folder)
        .map_err(|error| Error::io(format!("Cannot read {}", song_folder.display()), error))?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && !is_image(path))
        .filter(|path| {
            let is_artwork = |image: &PathBuf| same_file(path, image);
            !track_artwork.iter().any(|(_, image)| is_artwork(image))
                && !options.playlist_artwork.values().any(is_artwork)
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(Error::msg(format!("No songs in {}", song_folder.display())));
    }

    let mut songs = music::scan(&files);
    for (id, image) in &track_artwork {
        let Some(song) = songs.iter_mut().find(|song| &song.id() == id) else {
            return Err(Error::msg(format!("Artwork names an unknown track: {id}")));
        };
        song.artwork = Some(image.clone());
    }
    if let Some(name) = options.playlist_artwork.keys().find(|name| **name != options.playlist) {
        return Err(Error::msg(format!("Artwork names an unknown playlist: {name}")));
    }
    if let Some(name) = options.generated_playlist_artwork.iter().find(|name| **name != options.playlist) {
        return Err(Error::msg(format!("Generated artwork names an unknown playlist: {name}")));
    }

    let cancel = AtomicBool::new(false);
    let labels: Vec<String> = songs.iter().map(|song| song.id()).collect();
    let mut progress = |step: &music::Progress| match step.stage {
        Stage::Encoding => println!("{}", labels.get(step.song).map_or("", String::as_str)),
        Stage::Encoded => println!("  {}", step.detail),
        _ => {}
    };
    let result = music::pack(&options, &songs, &mut progress, &cancel)?;
    println!("{} song(s), {} KB of audio -> {}", result.songs, result.audio_kb, result.output.display());
    if !result.compressed {
        println!("note: assets are stored uncompressed (the game's Oodle codec is only usable on Windows)");
    }
    if args.thunderstore {
        let options = ThunderstoreOptions {
            author: args.author,
            version: args.version,
            readme_credit: !args.no_readme_credit,
            ..Default::default()
        };
        let zip = music::export_thunderstore(&result.output, &options)?;
        println!("Thunderstore package -> {}", zip.display());
    }
    Ok(())
}

pub fn run(arguments: Vec<OsString>) -> ExitCode {
    let args = match Args::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() { ExitCode::from(1) } else { ExitCode::SUCCESS };
        }
    };
    let result = match args.get_ffmpeg.clone() {
        Some(folder) => get_ffmpeg(folder),
        None => pack(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}
