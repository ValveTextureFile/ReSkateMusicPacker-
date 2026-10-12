// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The window's state and everything it does that is not drawing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dear_imgui_rs::texture::{OwnedTextureData, TextureFormat};
use dear_imgui_rs::{Context, ManagedTextureId};
use reskate_music::Error;
use reskate_music::music::{self, PackOptions, SongInfo, Stage, ThunderstoreOptions, ffmpeg_fetch, tools};
use reskate_music::settings::{Settings, is_game_folder};
use serde_json::Value;

use super::jobs::{Jobs, Message, Pixels, Reporter};

pub const BITRATES: [u32; 5] = [128, 160, 192, 256, 320];
pub const DEFAULT_BITRATE: usize = 2;
pub const AUDIO_FILTER: &[&str] = music::scan::AUDIO_EXTENSIONS;
pub const IMAGE_FILTER: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];

/// A song in the list.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub file: PathBuf,
    pub artist: String,
    pub title: String,
    /// Empty: the mod's default playlist.
    pub playlist: String,
    pub seconds: f64,
    /// From the scan; tag problems fall away once artist and title are filled in.
    pub problems: Vec<String>,
    pub scanned: bool,
    pub artwork: Option<PathBuf>,
    pub has_embedded_artwork: bool,
}

impl Row {
    pub fn id(&self) -> String {
        format!("{} - {}", self.artist, self.title)
    }
}

/// A song another mod or the game already has, for the clash warning.
pub struct ExternalSong {
    /// "mod 'KevinMacLeod'" or "the game soundtrack".
    pub source: String,
    /// The mod's folder; empty for the game.
    pub folder: Option<PathBuf>,
}

/// The cover being previewed: one at a time, replaced by each request.
#[derive(Default)]
pub struct Preview {
    pub label: String,
    pub texture: Option<ManagedTextureId>,
    pub loading: bool,
    pub error: String,
    generation: u64,
    upload: Option<Pixels>,
    retired: Vec<ManagedTextureId>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    NewMod,
    OpenMod,
}

pub struct ExportForm {
    pub author: String,
    pub version: String,
    pub description: String,
    pub icon: Option<PathBuf>,
    pub folder: Option<PathBuf>,
    pub reveal: bool,
    pub credit: bool,
}

/// Which dialogs to open this frame, and the state they keep while open.
#[derive(Default)]
pub struct Dialogs {
    pub settings: bool,
    pub artwork: bool,
    pub cover_prompt: bool,
    pub cover_prompt_index: usize,
    pub cover_popup_index: usize,
    pub export: bool,
    pub new_playlist: bool,
    pub new_playlist_name: String,
    pub new_playlist_row: Option<usize>,
    pub discard: Option<Pending>,
    pub context_row: Option<usize>,
}

pub struct App {
    pub settings: Settings,
    pub ffmpeg: bool,
    pub name: String,
    pub playlist: String,
    pub bitrate: usize,
    pub normalize: bool,
    pub playlist_artwork: BTreeMap<String, PathBuf>,
    pub generated_artwork: BTreeSet<String>,
    pub rows: Vec<Row>,
    /// Where the mod builds; `None` is `<game>/Mods/<name>`.
    pub output: Option<PathBuf>,
    pub status: Option<(String, bool)>,
    pub jobs: Jobs,
    pub preview: Preview,
    pub dialogs: Dialogs,
    pub export: ExportForm,
    /// The playlist tab showing; `None` is All.
    pub filter: Option<String>,
    /// A tab to select on the next frame (`Some(None)` is All).
    pub select_tab: Option<Option<String>>,
    pub custom_playlists: Vec<String>,
    pub external_songs: HashMap<String, ExternalSong>,
    pub game_running: Arc<AtomicBool>,
    pub dropped: Vec<PathBuf>,
}

/// A folder name from the mod's name: letters, digits, '-' and '_' ("Kevin MacLeod" -> KevinMacLeod).
pub fn folder_name(name: &str) -> String {
    let out: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    if out.is_empty() { "ReSkateMusic".into() } else { out }
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Whether the game is running, checked every few seconds off the UI thread.
fn watch_game(flag: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
        let mut system = System::new_with_specifics(RefreshKind::nothing());
        loop {
            system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
            let running = system.processes().values().any(|process| process.name().eq_ignore_ascii_case("Skate.exe"));
            flag.store(running, Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    });
}

impl App {
    pub fn new() -> Self {
        let settings = Settings::load();
        let (ffmpeg, _) = tools::locate(Some(&settings.ffmpeg));
        let game_running = Arc::new(AtomicBool::new(false));
        watch_game(Arc::clone(&game_running));
        let mut app = App {
            settings,
            ffmpeg,
            name: String::new(),
            playlist: String::new(),
            bitrate: DEFAULT_BITRATE,
            normalize: true,
            playlist_artwork: BTreeMap::new(),
            generated_artwork: BTreeSet::new(),
            rows: Vec::new(),
            output: None,
            status: None,
            jobs: Jobs::new(),
            preview: Preview::default(),
            dialogs: Dialogs::default(),
            export: ExportForm {
                author: "Author".into(),
                version: "1.0.0".into(),
                description: String::new(),
                icon: None,
                folder: None,
                reveal: true,
                credit: true,
            },
            filter: None,
            select_tab: None,
            custom_playlists: Vec::new(),
            external_songs: HashMap::new(),
            game_running,
            dropped: Vec::new(),
        };
        app.refresh_external_songs();
        app
    }

    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some((text.into(), error));
    }

    pub fn clear_status(&mut self) {
        self.status = None;
    }

    pub fn busy(&self) -> bool {
        self.jobs.busy()
    }

    pub fn has_game(&self) -> bool {
        is_game_folder(&self.settings.game)
    }

    /// The playlist songs go to when theirs is blank.
    pub fn default_playlist(&self) -> String {
        if self.playlist.is_empty() { "Default".into() } else { self.playlist.clone() }
    }

    pub fn row_playlist(&self, row: &Row) -> String {
        if row.playlist.is_empty() { self.default_playlist() } else { row.playlist.clone() }
    }

    /// Every playlist name that needs a cover decision: the default and any a song names.
    pub fn cover_playlists(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        if !self.playlist.is_empty() {
            names.insert(self.playlist.clone());
        }
        names.extend(self.rows.iter().filter(|row| !row.playlist.is_empty()).map(|row| row.playlist.clone()));
        names
    }

    pub fn has_cover_choice(&self, name: &str) -> bool {
        self.playlist_artwork.contains_key(name) || self.generated_artwork.contains(name)
    }

    pub fn output_folder(&self) -> PathBuf {
        self.output.clone().unwrap_or_else(|| self.settings.game.join("Mods").join(folder_name(&self.name)))
    }

    pub fn has_unsaved_changes(&self) -> bool {
        !self.rows.is_empty()
            || !self.name.is_empty()
            || !self.playlist.is_empty()
            || !self.custom_playlists.is_empty()
            || !self.playlist_artwork.is_empty()
            || !self.generated_artwork.is_empty()
    }

    // ---- settings ----------------------------------------------------------------------------

    pub fn choose_game_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new().set_title("The folder with Skate.exe").pick_folder() else { return };
        if is_game_folder(&folder) {
            self.settings.game = folder;
            self.save_settings();
            self.refresh_external_songs();
            self.clear_status();
        } else {
            self.set_status("That folder has no Skate.exe.", true);
        }
    }

    pub fn locate_ffmpeg(&mut self) {
        let Some(folder) = rfd::FileDialog::new().set_title("The folder with ffmpeg").pick_folder() else { return };
        self.use_ffmpeg_folder(&folder);
    }

    fn use_ffmpeg_folder(&mut self, folder: &Path) {
        let (found, taken) = tools::locate(Some(folder));
        if let (true, Some(taken)) = (found, taken) {
            self.settings.ffmpeg = taken;
            self.save_settings();
            self.ffmpeg = true;
            self.set_status("ffmpeg is ready.", false);
        } else {
            self.set_status(
                format!("That folder has no {} and {}.", tools::executable("ffmpeg"), tools::executable("ffprobe")),
                true,
            );
        }
    }

    fn save_settings(&mut self) {
        if let Err(error) = self.settings.save() {
            self.set_status(format!("Could not save the settings: {error}"), true);
        }
    }

    /// Downloads and installs the pinned ffmpeg build. Only ever starts from a click.
    pub fn download_ffmpeg(&mut self) {
        let Some((url, sha)) = ffmpeg_fetch::source() else { return };
        if self.busy() {
            return;
        }
        self.jobs.start("Downloading ffmpeg", move |job| {
            let result = (|| {
                let directory = ffmpeg_fetch::default_install_dir()?;
                let mut progress = |step: ffmpeg_fetch::DownloadProgress| {
                    let megabytes = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
                    if step.total != 0 {
                        let text = format!("{:.0} / {:.0} MB", megabytes(step.received), megabytes(step.total));
                        job.progress(step.received as f32 / step.total as f32, text);
                    } else {
                        job.progress(0.0, format!("{:.0} MB", megabytes(step.received)));
                    }
                };
                ffmpeg_fetch::ensure_ffmpeg(&directory, &url, &sha, &mut progress, job.cancel_flag())
            })();
            job.send(Message::Downloaded(result.map_err(|error| match error {
                Error::Cancelled => "Cancelled; ffmpeg was not installed.".to_owned(),
                other => other.to_string(),
            })));
        });
    }

    // ---- songs -------------------------------------------------------------------------------

    pub fn pick_songs(&mut self, folder: bool) {
        let dialog = rfd::FileDialog::new();
        let picked = if folder {
            dialog.set_title("A folder of songs").pick_folder().map(|folder| vec![folder])
        } else {
            dialog.add_filter("Audio", AUDIO_FILTER).add_filter("All files", &["*"]).pick_files()
        };
        if let Some(paths) = picked {
            self.add_files(&paths);
        }
    }

    pub fn pick_image() -> Option<PathBuf> {
        rfd::FileDialog::new().add_filter("Images", IMAGE_FILTER).add_filter("All files", &["*"]).pick_file()
    }

    /// Adds dropped or chosen files (folders bring in their audio files), into the playlist tab
    /// that is showing, and scans them in the background.
    pub fn add_files(&mut self, paths: &[PathBuf]) {
        if self.busy() {
            return;
        }
        let mut files = music::expand(paths);
        files.retain(|file| !self.rows.iter().any(|row| &row.file == file));
        if files.is_empty() {
            return;
        }
        let playlist = match &self.filter {
            Some(name) if *name != self.default_playlist() => name.clone(),
            _ => String::new(),
        };
        for file in &files {
            self.rows.push(Row { file: file.clone(), playlist: playlist.clone(), ..Default::default() });
        }
        self.scan(files);
    }

    fn scan(&mut self, files: Vec<PathBuf>) {
        self.jobs.start("Scanning", move |job| {
            for (index, file) in files.iter().enumerate() {
                if job.cancelled() {
                    break;
                }
                let info = music::scan_one(file);
                job.send(Message::Scanned(info));
                job.progress((index + 1) as f32 / files.len() as f32, file_name(file));
            }
        });
    }

    fn apply_scan(&mut self, mut info: SongInfo) {
        let Some(row) = self.rows.iter_mut().find(|row| row.file == info.file && !row.scanned) else { return };
        row.scanned = true;
        row.seconds = info.seconds;
        row.has_embedded_artwork = info.has_embedded_artwork;
        // Problems about the tags fall away once artist and title are filled in by hand.
        info.problems.retain(|problem| !problem.contains("tag"));
        row.problems = info.problems;
        if row.artist.is_empty() {
            row.artist = info.artist;
        }
        if row.title.is_empty() {
            row.title = info.title;
        }
    }

    /// Problems that block building one row; empty means fine.
    pub fn row_problems(&self, index: usize) -> Vec<String> {
        let row = &self.rows[index];
        let mut problems = row.problems.clone();
        if !music::usable_name(&row.artist) {
            problems.push("needs an artist".into());
        }
        if !music::usable_name(&row.title) {
            problems.push("needs a title".into());
        }
        if !row.playlist.is_empty() && !music::usable_name(&row.playlist) {
            problems.push("invalid playlist name".into());
        }
        let id = row.id();
        if let Some(other) = self.rows.iter().enumerate().position(|(other, song)| other != index && song.id() == id) {
            problems.push(format!("same artist and title as song {}", other + 1));
        }
        if !row.artist.is_empty()
            && !row.title.is_empty()
            && let Some(clash) = self.external_songs.get(&id.to_lowercase())
        {
            let this_mod = self.output_folder();
            let same_mod = clash.folder.as_ref().is_some_and(|folder| same_folder(folder, &this_mod));
            if !same_mod {
                problems.push(format!("already in {}", clash.source));
            }
        }
        problems
    }

    pub fn move_row(&mut self, from: usize, to: usize) {
        if from < self.rows.len() && to < self.rows.len() {
            self.rows.swap(from, to);
        }
    }

    // ---- mods --------------------------------------------------------------------------------

    pub fn reset(&mut self) {
        self.rows.clear();
        self.output = None;
        self.filter = None;
        self.custom_playlists.clear();
        self.select_tab = None;
        self.name.clear();
        self.playlist.clear();
        self.normalize = true;
        self.bitrate = DEFAULT_BITRATE;
        self.playlist_artwork.clear();
        self.generated_artwork.clear();
        self.clear_preview();
        self.clear_status();
    }

    pub fn open_mod_dialog(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().set_title("A mod built by this program").pick_folder() {
            self.open_mod(&folder);
        }
    }

    pub fn open_mod(&mut self, folder: &Path) {
        if self.busy() {
            return;
        }
        let project = match music::load_project(folder) {
            Ok(project) => project,
            Err(error) => return self.set_status(error.to_string(), true),
        };
        self.reset();
        self.name = project.name;
        self.playlist = project.playlist;
        self.bitrate = BITRATES.iter().position(|&rate| rate == project.bitrate).unwrap_or(DEFAULT_BITRATE);
        self.normalize = project.normalize;
        self.playlist_artwork = project.playlist_artwork;
        self.generated_artwork = project.generated_playlist_artwork;
        self.output = Some(folder.to_path_buf());
        self.refresh_external_songs();
        let files = project.songs.iter().map(|song| song.file.clone()).collect();
        self.rows = project
            .songs
            .into_iter()
            .map(|song| Row {
                file: song.file,
                artist: song.artist,
                title: song.title,
                playlist: song.playlist,
                artwork: song.artwork,
                ..Default::default()
            })
            .collect();
        // Scan for lengths and problems; the project's artist and title stay.
        self.scan(files);
        self.set_status(format!("Opened {}.", file_name(folder)), false);
    }

    /// Songs already in other installed mods and in the game's soundtrack, for the clash warning.
    pub fn refresh_external_songs(&mut self) {
        self.external_songs.clear();
        if !self.has_game() {
            return;
        }
        let read_json =
            |path: &Path| std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
        if let Ok(entries) = std::fs::read_dir(self.settings.game.join("Mods")) {
            for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
                let folder = entry.path();
                let source =
                    || ExternalSong { source: format!("mod '{}'", file_name(&folder)), folder: Some(folder.clone()) };
                if let Some(root) = read_json(&folder.join("reskate-music.json")) {
                    for playlist in root.get("playlists").and_then(Value::as_array).into_iter().flatten() {
                        for song in playlist
                            .get("songs")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            self.external_songs.entry(song.to_lowercase()).or_insert_with(source);
                        }
                    }
                }
                if let Some(root) = read_json(&folder.join("reskate-music-project.json")) {
                    for song in root.get("songs").and_then(Value::as_array).into_iter().flatten() {
                        let field = |key: &str| song.get(key).and_then(Value::as_str).unwrap_or_default();
                        if !field("artist").is_empty() && !field("title").is_empty() {
                            let id = format!("{} - {}", field("artist"), field("title")).to_lowercase();
                            self.external_songs.entry(id).or_insert_with(source);
                        }
                    }
                }
            }
        }
        // ReSkate's content cache holds the shipped songs' "Artist - Title" ids as short strings.
        if let Some(cache) = dirs::cache_dir().map(|dir| dir.join("ReSkate").join("cache")) {
            let mut files = Vec::new();
            collect_cache_files(&cache, &mut files);
            for file in files {
                let Ok(buffer) = std::fs::read(&file) else { continue };
                for id in cached_song_ids(&buffer) {
                    self.external_songs
                        .entry(id.to_lowercase())
                        .or_insert_with(|| ExternalSong { source: "the game soundtrack".into(), folder: None });
                }
            }
        }
    }

    /// Why the build button is disabled, if it is.
    pub fn build_blocker(&self) -> Option<String> {
        if !music::usable_name(&self.name) {
            return Some("Give the mod a name.".into());
        }
        if !music::usable_name(&self.playlist) {
            return Some("Give the playlist a name.".into());
        }
        let blocked = (0..self.rows.len()).filter(|&index| !self.row_problems(index).is_empty()).count();
        if blocked != 0 {
            return Some(format!("{blocked} song(s) need fixing."));
        }
        if self.rows.iter().any(|row| !row.scanned) {
            return Some("Reading the songs...".into());
        }
        None
    }

    /// The build button: first asks about any playlist still on an automatic cover.
    pub fn request_build(&mut self) {
        self.clear_status();
        if self.cover_playlists().iter().any(|name| !self.has_cover_choice(name)) {
            self.dialogs.cover_prompt = true;
        } else {
            self.build();
        }
    }

    pub fn build(&mut self) {
        if self.busy() {
            return;
        }
        let options = PackOptions {
            game: self.settings.game.clone(),
            output: self.output_folder(),
            name: self.name.clone(),
            playlist: self.playlist.clone(),
            bitrate: BITRATES[self.bitrate],
            normalize: self.normalize,
            playlist_artwork: self.playlist_artwork.clone(),
            generated_playlist_artwork: self.generated_artwork.clone(),
        };
        let songs: Vec<SongInfo> = self
            .rows
            .iter()
            .map(|row| SongInfo {
                file: row.file.clone(),
                artist: row.artist.clone(),
                title: row.title.clone(),
                playlist: row.playlist.clone(),
                artwork: row.artwork.clone(),
                ..Default::default()
            })
            .collect();
        self.jobs.start("Building", move |job| {
            let mut progress = |step: &music::Progress| {
                let done = step.song as f32 + if step.stage == Stage::Encoded { 1.0 } else { 0.0 };
                let text = if step.song < step.count {
                    format!("{} song {} of {}", step.stage.label(), step.song + 1, step.count)
                } else {
                    format!("{} the mod", step.stage.label())
                };
                job.progress((done / (step.count + 1) as f32).min(1.0), text);
            };
            let result = music::pack(&options, &songs, &mut progress, job.cancel_flag());
            job.send(Message::Built(result.map_err(|error| match error {
                Error::Cancelled => "Cancelled; nothing was written.".to_owned(),
                other => other.to_string(),
            })));
        });
    }

    pub fn export_thunderstore(&mut self) {
        if self.busy() {
            return;
        }
        let folder = self.output_folder();
        if !folder.join("layout.toc").exists() {
            return self.set_status("Build the mod first before exporting.", true);
        }
        let options = ThunderstoreOptions {
            author: self.export.author.clone(),
            version: self.export.version.clone(),
            description: self.export.description.clone(),
            icon: self.export.icon.clone(),
            output: Some(
                self.export.folder.clone().unwrap_or_else(|| folder.parent().unwrap_or(&folder).to_path_buf()),
            ),
            readme_credit: self.export.credit,
        };
        self.jobs.start("Exporting", move |job| {
            job.progress(0.0, "packaging the mod");
            let result = music::export_thunderstore(&folder, &options).map_err(|error| error.to_string());
            job.send(Message::Exported(result));
        });
    }

    // ---- covers ------------------------------------------------------------------------------

    pub fn clear_preview(&mut self) {
        let preview = &mut self.preview;
        preview.generation += 1; // results still being prepared are dropped
        preview.loading = false;
        preview.label.clear();
        preview.error.clear();
        preview.upload = None;
        if let Some(texture) = preview.texture.take() {
            preview.retired.push(texture);
        }
    }

    /// Prepares a cover preview off the UI thread: a chosen image, a generated text cover, or the
    /// first embedded album art among `tracks`.
    pub fn request_preview(
        &mut self,
        label: &str,
        source: Option<PathBuf>,
        generated: bool,
        tracks: Vec<(PathBuf, Option<PathBuf>)>,
    ) {
        if self.busy() {
            return;
        }
        self.clear_preview();
        self.preview.label = label.to_owned();
        self.preview.loading = true;
        let generation = self.preview.generation;
        let label = label.to_owned();
        self.jobs.start("Loading artwork", move |job: Reporter| {
            let result = (|| -> reskate_music::Result<Pixels> {
                let png = if generated {
                    music::playlist_artwork_png(&label)?
                } else {
                    let mut source = source;
                    for (file, artwork) in &tracks {
                        if source.is_some() || job.cancelled() {
                            break;
                        }
                        source = artwork.clone().or_else(|| music::embedded_artwork(file));
                    }
                    let Some(source) = source else { return Err(Error::msg("No artwork available for this cover.")) };
                    music::image_artwork_png(&source)?
                };
                music::artwork::decode_png_rgba(&png)
            })();
            job.send(Message::Preview { generation, result: result.map_err(|error| error.to_string()) });
        });
    }

    pub fn preview_playlist(&mut self, name: &str) {
        let source = self.playlist_artwork.get(name).cloned();
        let generated = self.generated_artwork.contains(name);
        let tracks = self
            .rows
            .iter()
            .filter(|row| self.row_playlist(row) == name)
            .map(|row| (row.file.clone(), row.artwork.clone()))
            .collect();
        self.request_preview(name, source, generated, tracks);
    }

    pub fn set_playlist_image(&mut self, name: &str, image: PathBuf) {
        self.playlist_artwork.insert(name.to_owned(), image);
        self.generated_artwork.remove(name);
        self.preview_playlist(name);
    }

    pub fn set_playlist_generated(&mut self, name: &str) {
        self.generated_artwork.insert(name.to_owned());
        self.playlist_artwork.remove(name);
        self.preview_playlist(name);
    }

    pub fn set_playlist_automatic(&mut self, name: &str) {
        self.playlist_artwork.remove(name);
        self.generated_artwork.remove(name);
        self.preview_playlist(name);
    }

    // ---- per frame ---------------------------------------------------------------------------

    /// Finished jobs, dropped files and texture uploads; runs before each frame is built.
    pub fn update(&mut self, context: &mut Context) {
        for message in self.jobs.drain() {
            match message {
                Message::Progress { .. } => {}
                Message::Scanned(info) => self.apply_scan(info),
                Message::Built(Ok(result)) => {
                    self.set_status(
                        format!(
                            "Built {} song(s) into {}. Start the game through the ReSkate launcher to hear them.",
                            result.songs,
                            result.output.display()
                        ),
                        false,
                    );
                    self.refresh_external_songs();
                }
                Message::Built(Err(error)) | Message::Exported(Err(error)) => self.set_status(error, true),
                Message::Exported(Ok(zip)) => {
                    self.set_status(format!("Exported: {}", zip.display()), false);
                    if self.export.reveal {
                        let _ = opener::reveal(&zip);
                    }
                }
                Message::Downloaded(Ok(folder)) => self.use_ffmpeg_folder(&folder),
                Message::Downloaded(Err(error)) => self.set_status(error, true),
                Message::Preview { generation, result } => {
                    if generation == self.preview.generation {
                        self.preview.loading = false;
                        match result {
                            Ok(pixels) => self.preview.upload = Some(pixels),
                            Err(error) => self.preview.error = error,
                        }
                    }
                }
            }
        }
        for texture in self.preview.retired.drain(..) {
            let _ = context.remove_texture(texture);
        }
        if let Some((pixels, width, height)) = self.preview.upload.take() {
            match OwnedTextureData::from_pixels(TextureFormat::RGBA32, width, height, &pixels) {
                Ok(texture) => self.preview.texture = Some(context.register_texture(texture)),
                Err(error) => self.preview.error = format!("Could not display the cover preview: {error}"),
            }
        }
        if !self.dropped.is_empty() {
            let dropped = std::mem::take(&mut self.dropped);
            if self.has_game() && self.ffmpeg {
                self.add_files(&dropped);
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.jobs.shutdown();
    }
}

fn same_folder(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn collect_cache_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_cache_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "cache") {
            out.push(path);
        }
    }
}

/// Printable "Artist - Title" strings in a ReSkate cache file: a 0x12 tag, a 1- or 2-byte
/// varint length, then the text.
fn cached_song_ids(buffer: &[u8]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut i = 0;
    while i + 4 < buffer.len() {
        if buffer[i] == 0x12 {
            let mut length = buffer[i + 1] as usize;
            let mut header = 2;
            if length & 0x80 != 0 {
                length = (length & 0x7F) | (((buffer[i + 2] & 0x7F) as usize) << 7);
                header = 3;
            }
            if (5..200).contains(&length) && i + header + length <= buffer.len() {
                let text = &buffer[i + header..i + header + length];
                if text.iter().all(|&byte| (32..=126).contains(&byte)) {
                    let text = String::from_utf8_lossy(text);
                    if text.contains(" - ") {
                        ids.push(text.into_owned());
                    }
                }
            }
        }
        i += 1;
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_drop_punctuation() {
        assert_eq!(folder_name("Kevin MacLeod"), "KevinMacLeod");
        assert_eq!(folder_name("!!"), "ReSkateMusic");
    }

    #[test]
    fn cache_ids_are_tagged_strings() {
        let mut buffer = vec![0u8, 0x12, 9];
        buffer.extend_from_slice(b"A - Title");
        buffer.extend_from_slice(&[0x12, 3, b'x', b'y', b'z', 0, 0]);
        assert_eq!(cached_song_ids(&buffer), ["A - Title"]);
    }
}
