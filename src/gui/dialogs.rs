// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Popups and modal dialogs.

use dear_imgui_rs::*;
use reskate_music::music::{self, tools};

use super::app::{App, Pending, file_name};
use super::theme;
use super::view::{Playlists, colored_button, ffmpeg_download_controls, tooltip};

pub fn draw(app: &mut App, ui: &Ui) {
    settings(app, ui);
    artwork_manager(app, ui);
    new_playlist(app, ui);
    discard(app, ui);
    cover_prompt(app, ui);
    export(app, ui);
}

/// Opens the named modal when its flag is set, then begins it.
fn modal<'ui>(ui: &'ui Ui, name: &'ui str, open: &mut bool) -> Option<ModalPopupToken<'ui>> {
    if std::mem::take(open) {
        ui.open_popup(name);
    }
    ui.begin_modal_popup_config(name).flags(WindowFlags::ALWAYS_AUTO_RESIZE).begin()
}

/// The cover preview box: the image, or why there is none.
fn preview_box(app: &App, ui: &Ui, label: &str, size: f32) {
    if app.preview.loading && app.preview.label == label {
        ui.child_window("preview_box").size([size, size]).child_flags(ChildFlags::BORDERS).build(ui, || {
            ui.text_disabled("Loading preview...");
        });
    } else if let Some(texture) = app.preview.texture.filter(|_| app.preview.label == label) {
        ui.image(texture, [size, size]);
    } else {
        ui.child_window("preview_box").size([size, size]).child_flags(ChildFlags::BORDERS).build(ui, || {
            ui.spacing();
            ui.text_disabled("No cover");
            if !app.preview.error.is_empty() && app.preview.label == label {
                ui.spacing();
                let _wrap = ui.push_text_wrap_pos(ui.cursor_pos_x() + size - 20.0);
                ui.text_colored(theme::ERROR, &app.preview.error);
            }
        });
    }
}

fn active_cover(app: &App, ui: &Ui, name: &str) {
    if let Some(image) = app.playlist_artwork.get(name) {
        ui.text(format!("Active: {}", file_name(image)));
    } else if app.generated_artwork.contains(name) {
        ui.text("Active: Generated text cover");
    } else {
        ui.text("Active: Automatic (from the first track)");
    }
}

/// The toolbar's cover button popup: pick a playlist, then an image, a text cover or automatic.
pub fn cover_popup(app: &mut App, ui: &Ui, playlists: &Playlists) {
    let Some(_popup) = ui.begin_popup("cover_popup") else { return };
    let busy = app.busy();
    if app.dialogs.cover_popup_index >= playlists.names.len() {
        app.dialogs.cover_popup_index = 0;
    }
    let mut changed = false;
    if playlists.names.len() > 1 {
        ui.set_next_item_width(200.0);
        changed = ui.combo_simple_string("Playlist##cover", &mut app.dialogs.cover_popup_index, &playlists.names);
        ui.separator();
    }
    let name = playlists.names[app.dialogs.cover_popup_index].clone();
    let default = name == app.default_playlist();
    ui.text(format!("Playlist: {name}{}", if default { " (default)" } else { "" }));
    ui.separator();
    active_cover(app, ui, &name);

    let artwork_busy = busy || app.preview.loading;
    if changed || (app.preview.label != name && !artwork_busy) {
        app.preview_playlist(&name);
    }
    ui.spacing();
    preview_box(app, ui, &name, 128.0);
    ui.spacing();

    {
        let _disabled = ui.begin_disabled_with_cond(artwork_busy);
        if ui.button("Choose Image...")
            && let Some(image) = App::pick_image()
        {
            app.set_playlist_image(&name, image);
        }
        let label =
            if app.generated_artwork.contains(&name) { "Re-generate Text Cover" } else { "Generate Text Cover" };
        if ui.button(label) {
            app.set_playlist_generated(&name);
        }
        if ui.button("Use Automatic") {
            app.set_playlist_automatic(&name);
        }
        if playlists.names.len() > 1 {
            ui.spacing();
            if ui.button("Generate Text for All Playlists") {
                for playlist in &playlists.names {
                    app.generated_artwork.insert(playlist.clone());
                    app.playlist_artwork.remove(playlist);
                }
                app.preview_playlist(&name);
            }
        }
    }
    ui.separator();
    if ui.button("Manage Track Artwork...") {
        app.dialogs.artwork = true;
        ui.close_current_popup();
    }
    ui.same_line();
    if ui.button("Close") {
        ui.close_current_popup();
    }
}

fn settings(app: &mut App, ui: &Ui) {
    let Some(_modal) = modal(ui, "Settings", &mut app.dialogs.settings) else { return };
    let _wrap = ui.push_text_wrap_pos(ui.cursor_pos_x() + 540.0);
    ui.text_colored(theme::ACCENT, "Game folder");
    ui.text_wrapped(if app.settings.game.as_os_str().is_empty() {
        "Not set.".to_owned()
    } else {
        app.settings.game.display().to_string()
    });
    if ui.button_with_size("Choose game folder...", [190.0, 28.0]) {
        app.choose_game_folder();
    }
    ui.separator();
    ui.text_colored(theme::ACCENT, "ffmpeg");
    let (ffmpeg, ffprobe) = (tools::executable("ffmpeg"), tools::executable("ffprobe"));
    ui.text_wrapped(if app.ffmpeg {
        format!("{ffmpeg} and {ffprobe} found.")
    } else {
        format!("Not found - the packer needs {ffmpeg} and {ffprobe}.")
    });
    if !app.settings.ffmpeg.as_os_str().is_empty() {
        ui.text_disabled(app.settings.ffmpeg.display().to_string());
    }
    if !app.ffmpeg {
        ffmpeg_download_controls(app, ui, 250.0);
        ui.spacing();
    }
    if ui.button_with_size("Open the ffmpeg download page", [240.0, 28.0]) {
        let _ = opener::open("https://ffmpeg.org/download.html");
    }
    ui.same_line();
    if ui.button_with_size("Locate ffmpeg...", [150.0, 28.0]) {
        app.locate_ffmpeg();
    }
    ui.separator();
    if ui.button_with_size("Close", [120.0, 28.0]) {
        ui.close_current_popup();
    }
}

fn artwork_manager(app: &mut App, ui: &Ui) {
    let Some(_modal) = modal(ui, "Track and playlist artwork", &mut app.dialogs.artwork) else { return };
    {
        let _wrap = ui.push_text_wrap_pos(ui.cursor_pos_x() + 700.0);
        ui.text_wrapped(
            "Track covers use embedded album art unless you choose an image. Playlists use their first track's \
             cover, or you can choose an image or generate a text cover.",
        );
    }
    let busy = app.busy();
    {
        let _disabled = ui.begin_disabled_with_cond(busy);
        ui.child_window("covers").size([480.0, 360.0]).build(ui, || {
            ui.text("Playlists");
            for name in app.cover_playlists() {
                let _id = ui.push_id(&format!("playlist {name}"));
                ui.text_wrapped(&name);
                match app.playlist_artwork.get(&name) {
                    Some(image) => {
                        ui.text_disabled(file_name(image));
                        tooltip(ui, &image.display().to_string());
                    }
                    None => ui.text_disabled("Automatic cover"),
                }
                if ui.button("Choose image...")
                    && let Some(image) = App::pick_image()
                {
                    app.set_playlist_image(&name, image);
                }
                ui.same_line();
                if ui.button("Use automatic") {
                    app.set_playlist_automatic(&name);
                }
                let mut generated = app.generated_artwork.contains(&name);
                if ui.checkbox("Generate text cover", &mut generated) {
                    if generated {
                        app.set_playlist_generated(&name);
                    } else {
                        app.generated_artwork.remove(&name);
                        app.preview_playlist(&name);
                    }
                }
                if ui.button("Preview") {
                    app.preview_playlist(&name);
                }
                ui.separator();
            }
            ui.text("Tracks");
            for index in 0..app.rows.len() {
                let _id = ui.push_id(index as i32);
                let id = app.rows[index].id();
                ui.text_wrapped(&id);
                match &app.rows[index].artwork {
                    Some(image) => {
                        ui.text_disabled(file_name(image));
                        tooltip(ui, &image.display().to_string());
                    }
                    None => ui.text_disabled(if app.rows[index].has_embedded_artwork {
                        "Embedded album art detected"
                    } else {
                        "No embedded album art detected"
                    }),
                }
                if ui.button("Choose image...")
                    && let Some(image) = App::pick_image()
                {
                    app.rows[index].artwork = Some(image.clone());
                    app.request_preview(&id, Some(image), false, Vec::new());
                }
                ui.same_line();
                if ui.button("Use automatic") {
                    app.rows[index].artwork = None;
                    app.clear_preview();
                }
                ui.same_line();
                if ui.button("Preview") {
                    let row = &app.rows[index];
                    let tracks = vec![(row.file.clone(), None)];
                    app.request_preview(&id, row.artwork.clone(), false, tracks);
                }
                ui.separator();
            }
        });
        ui.same_line();
        ui.child_window("cover_preview").size([220.0, 360.0]).build(ui, || {
            ui.text_wrapped(if app.preview.label.is_empty() { "Cover preview" } else { &app.preview.label });
            if app.preview.loading {
                ui.text_wrapped("Loading artwork...");
            } else if let Some(texture) = app.preview.texture {
                ui.image(texture, [200.0, 200.0]);
            } else {
                ui.text_wrapped("Choose a cover or click Preview.");
            }
            if !app.preview.error.is_empty() {
                ui.text_colored(theme::ERROR, &app.preview.error);
            }
        });
    }
    if ui.button_with_size("Done", [120.0, 0.0]) {
        ui.close_current_popup();
    }
}

fn new_playlist(app: &mut App, ui: &Ui) {
    let Some(_modal) = modal(ui, "New Playlist", &mut app.dialogs.new_playlist) else { return };
    ui.text("Enter a name for the new playlist:");
    ui.spacing();
    ui.set_next_item_width(260.0);
    if ui.is_window_appearing() {
        ui.set_keyboard_focus_here();
    }
    let entered = ui.input_text("##new_playlist", &mut app.dialogs.new_playlist_name).enter_returns_true(true).build();
    ui.spacing();
    let name = app.dialogs.new_playlist_name.trim().to_owned();
    let valid = music::usable_name(&name);
    let create = {
        let _disabled = ui.begin_disabled_with_cond(!valid);
        ui.button_with_size("Create", [100.0, 0.0])
    };
    if (create || entered) && valid {
        if !app.custom_playlists.contains(&name) {
            app.custom_playlists.push(name.clone());
        }
        if let Some(row) = app.dialogs.new_playlist_row.take().filter(|&row| row < app.rows.len()) {
            app.rows[row].playlist = name.clone();
        }
        app.filter = Some(name.clone());
        app.select_tab = Some(Some(name));
        ui.close_current_popup();
    }
    ui.same_line();
    if ui.button_with_size("Cancel", [80.0, 0.0]) || ui.is_key_pressed(Key::Escape) {
        app.dialogs.new_playlist_row = None;
        ui.close_current_popup();
    }
}

fn discard(app: &mut App, ui: &Ui) {
    let mut open = app.dialogs.discard.is_some() && !ui.is_popup_open("Discard Changes?");
    let Some(_modal) = modal(ui, "Discard Changes?", &mut open) else { return };
    {
        let _wrap = ui.push_text_wrap_pos(ui.cursor_pos_x() + 380.0);
        ui.text_wrapped("You have songs or settings loaded. Starting a new mod or opening another discards them.");
    }
    ui.spacing();
    ui.separator();
    ui.spacing();
    if ui.button_with_size("Discard and Continue", [170.0, 0.0]) {
        let pending = app.dialogs.discard.take();
        ui.close_current_popup();
        match pending {
            Some(Pending::NewMod) => app.reset(),
            Some(Pending::OpenMod) => app.open_mod_dialog(),
            None => {}
        }
    }
    ui.same_line();
    if ui.button_with_size("Cancel", [80.0, 0.0]) || ui.is_key_pressed(Key::Escape) {
        app.dialogs.discard = None;
        ui.close_current_popup();
    }
}

/// Asked before a build when a playlist would get an automatic cover.
fn cover_prompt(app: &mut App, ui: &Ui) {
    if app.dialogs.cover_prompt {
        let first = app
            .cover_playlists()
            .into_iter()
            .find(|name| !app.has_cover_choice(name))
            .unwrap_or_else(|| "Playlist".into());
        app.dialogs.cover_prompt_index = 0;
        app.preview_playlist(&first);
    }
    let Some(_modal) = modal(ui, "Playlist Artwork Setup", &mut app.dialogs.cover_prompt) else { return };
    let unconfigured: Vec<String> =
        app.cover_playlists().into_iter().filter(|name| !app.has_cover_choice(name)).collect();
    if app.dialogs.cover_prompt_index >= unconfigured.len() {
        app.dialogs.cover_prompt_index = 0;
    }
    let name = unconfigured
        .get(app.dialogs.cover_prompt_index)
        .cloned()
        .unwrap_or_else(|| if app.playlist.is_empty() { "Playlist".into() } else { app.playlist.clone() });

    ui.spacing();
    ui.text("Playlist artwork has not been chosen.");
    ui.text_disabled("skate. shows playlist covers in its in-game music menu.");
    ui.spacing();
    ui.group(|| preview_box(app, ui, &name, 180.0));
    ui.same_line();
    ui.spacing();
    ui.same_line();
    let artwork_busy = app.busy() || app.preview.loading;
    ui.group(|| {
        if unconfigured.len() > 1 {
            let _disabled = ui.begin_disabled_with_cond(artwork_busy);
            if ui.combo_simple_string("Playlist", &mut app.dialogs.cover_prompt_index, &unconfigured) {
                let current = unconfigured[app.dialogs.cover_prompt_index].clone();
                app.preview_playlist(&current);
            }
        } else {
            ui.text(format!("Playlist: {name}"));
        }
        ui.spacing();
        active_cover(app, ui, &name);
        ui.spacing();
        let _disabled = ui.begin_disabled_with_cond(artwork_busy);
        if ui.button_with_size("Generate Text Cover", [200.0, 28.0]) {
            app.set_playlist_generated(&name);
        }
        if ui.button_with_size("Choose Image...", [200.0, 28.0])
            && let Some(image) = App::pick_image()
        {
            app.set_playlist_image(&name, image);
        }
        if ui.button_with_size("Use Automatic", [200.0, 28.0]) {
            app.set_playlist_automatic(&name);
        }
        if unconfigured.len() > 1 {
            ui.spacing();
            if ui.button_with_size("Generate Text for All", [200.0, 28.0]) {
                for playlist in &unconfigured {
                    app.generated_artwork.insert(playlist.clone());
                }
                app.preview_playlist(&name);
            }
        }
    });
    ui.spacing();
    ui.separator();
    ui.spacing();
    let build = {
        let _disabled = ui.begin_disabled_with_cond(artwork_busy);
        colored_button(ui, "Build", [120.0, 30.0], theme::PRIMARY)
    };
    if build {
        app.clear_preview();
        app.build();
        ui.close_current_popup();
    }
    ui.same_line();
    if ui.button_with_size("Cancel", [100.0, 30.0]) {
        app.clear_preview();
        ui.close_current_popup();
    }
}

fn export(app: &mut App, ui: &Ui) {
    let Some(_modal) = modal(ui, "Export Thunderstore Package", &mut app.dialogs.export) else { return };
    ui.spacing();
    ui.text("Export a Thunderstore-compatible .zip package ready for upload.");
    ui.spacing();
    for (label, value) in [
        ("Author / Namespace", &mut app.export.author),
        ("Version", &mut app.export.version),
        ("Description", &mut app.export.description),
    ] {
        ui.set_next_item_width(320.0);
        ui.input_text(label, value).build();
    }
    ui.spacing();
    match app.export.icon.clone() {
        Some(icon) => {
            ui.text(format!("Icon: {}", file_name(&icon)));
            tooltip(ui, &icon.display().to_string());
            ui.same_line();
            if ui.button("Clear##icon") {
                app.export.icon = None;
            }
        }
        None => {
            ui.text_disabled("Icon: default (generated 256x256)");
            ui.same_line();
            if ui.button("Browse...##icon") {
                app.export.icon = App::pick_image().or(app.export.icon.take());
            }
        }
    }
    ui.spacing();
    let mod_folder = app.output_folder();
    let folder = app.export.folder.clone().unwrap_or_else(|| mod_folder.parent().unwrap_or(&mod_folder).to_path_buf());
    ui.text(format!("Output folder: {}", file_name(&folder)));
    tooltip(ui, &folder.display().to_string());
    ui.same_line();
    if ui.button("Browse...##output")
        && let Some(picked) = rfd::FileDialog::new().pick_folder()
    {
        app.export.folder = Some(picked);
    }
    if app.export.folder.is_some() {
        ui.same_line();
        if ui.button("Reset##output") {
            app.export.folder = None;
        }
    }
    let name = |text: &str, fallback: &str| music::thunderstore_name(if text.is_empty() { fallback } else { text });
    let version = if app.export.version.is_empty() { "1.0.0" } else { &app.export.version };
    ui.text_disabled(format!(
        "Package: {}-{}-{version}.zip",
        name(&app.export.author, "Author"),
        name(&app.name, "ReSkateMusic")
    ));
    ui.spacing();
    ui.checkbox("Show the package when finished", &mut app.export.reveal);
    ui.checkbox("Include a \"Packaged with ReSkate Music Packer\" link in the README", &mut app.export.credit);
    ui.spacing();
    ui.separator();
    ui.spacing();
    if colored_button(ui, "Export ZIP", [140.0, 30.0], theme::PRIMARY) {
        app.export_thunderstore();
        ui.close_current_popup();
    }
    ui.same_line();
    if ui.button_with_size("Cancel", [100.0, 30.0]) {
        ui.close_current_popup();
    }
}
