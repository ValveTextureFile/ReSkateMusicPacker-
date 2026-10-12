// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The main window: first-run setup, then the song list with its toolbar and footer.

use std::sync::atomic::Ordering;

use dear_imgui_rs::*;
use reskate_music::music::{ffmpeg_fetch, tools};

use super::app::{App, BITRATES, Pending, file_name};
use super::{dialogs, theme};

const DOWNLOAD_PAGE: &str = "https://ffmpeg.org/download.html";

pub fn colored_button(ui: &Ui, label: &str, size: [f32; 2], colors: [[f32; 4]; 3]) -> bool {
    let _normal = ui.push_style_color(StyleColor::Button, colors[0]);
    let _hovered = ui.push_style_color(StyleColor::ButtonHovered, colors[1]);
    let _active = ui.push_style_color(StyleColor::ButtonActive, colors[2]);
    ui.button_with_size(label, size)
}

pub fn tooltip(ui: &Ui, text: &str) {
    if ui.is_item_hovered() {
        ui.tooltip_text(text);
    }
}

/// Puts the next item of `width` in the middle of the space left on the line.
fn center_next(ui: &Ui, width: f32) {
    let available = ui.content_region_avail()[0];
    if available > width {
        ui.set_cursor_pos_x(ui.cursor_pos_x() + (available - width) * 0.5);
    }
}

/// `text` shortened with "..." in the middle until it fits `width`.
fn elide_middle(ui: &Ui, text: &str, width: f32) -> String {
    if ui.calc_text_size(text)[0] <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 4 {
        keep -= 1;
        let head: String = chars[..keep / 2].iter().collect();
        let tail: String = chars[chars.len() - (keep - keep / 2)..].iter().collect();
        let candidate = format!("{head}...{tail}");
        if ui.calc_text_size(&candidate)[0] <= width {
            return candidate;
        }
    }
    "...".into()
}

fn centered_text(ui: &Ui, text: &str, color: Option<[f32; 4]>) {
    center_next(ui, ui.calc_text_size(text)[0]);
    match color {
        Some(color) => ui.text_colored(color, text),
        None => ui.text_disabled(text),
    }
}

pub fn duration(seconds: f64) -> String {
    let total = seconds as u64;
    let (hours, minutes, seconds) = (total / 3600, total % 3600 / 60, total % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

pub fn draw(app: &mut App, ui: &Ui) {
    let size = ui.io().display_size();
    ui.window("##main")
        .position([0.0, 0.0], Condition::Always)
        .size(size, Condition::Always)
        .flags(
            WindowFlags::NO_DECORATION
                | WindowFlags::NO_MOVE
                | WindowFlags::NO_SAVED_SETTINGS
                | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS,
        )
        .build(|| {
            if !app.has_game() {
                setup_page(app, ui);
            } else if !app.ffmpeg {
                ffmpeg_page(app, ui);
            } else {
                songs_page(app, ui);
            }
            if (!app.has_game() || !app.ffmpeg)
                && let Some((text, error)) = &app.status
            {
                ui.spacing();
                ui.text_colored(if *error { theme::ERROR } else { theme::SUCCESS }, text);
            }
            dialogs::draw(app, ui);
        });
}

fn setup_page(app: &mut App, ui: &Ui) {
    ui.spacing();
    ui.text_colored(theme::ACCENT, "Game Setup");
    ui.spacing();
    ui.text_wrapped("Pick the folder skate. is installed in (the one with Skate.exe and ReSkateLauncher.exe).");
    ui.spacing();
    if colored_button(ui, "Choose the game folder...", [240.0, 34.0], theme::PRIMARY) {
        app.choose_game_folder();
    }
}

/// The one-click installer, shared by first-run setup and the Settings dialog. Downloading only
/// ever starts from a click here; the source, size and licence sit under the button.
pub fn ffmpeg_download_controls(app: &mut App, ui: &Ui, width: f32) {
    if ffmpeg_fetch::source().is_none() {
        ui.text_wrapped(if cfg!(target_os = "macos") {
            "Install it with Homebrew (brew install ffmpeg), then click Locate ffmpeg or restart this program."
        } else {
            "Install ffmpeg with your package manager, then click Locate ffmpeg or restart this program."
        });
        return;
    }
    if app.jobs.label() == Some("Downloading ffmpeg") {
        let label = if app.jobs.text.is_empty() { "Starting...".to_owned() } else { app.jobs.text.clone() };
        ui.progress_bar(app.jobs.fraction).size([width, 26.0]).overlay_text(label).build();
        ui.same_line();
        if ui.button_with_size("Cancel", [-1.0, 26.0]) {
            app.jobs.cancel();
        }
        return;
    }
    let clicked = {
        let _disabled = ui.begin_disabled_with_cond(app.busy());
        colored_button(ui, "Download ffmpeg automatically", [width, 32.0], theme::PRIMARY)
    };
    ui.text_disabled(
        "Static LGPL build from BtbN/FFmpeg-Builds on GitHub (~163 MB). Downloaded only when you click, then \
         checked against a pinned SHA-256.",
    );
    if clicked {
        app.download_ffmpeg();
    }
}

fn ffmpeg_page(app: &mut App, ui: &Ui) {
    ui.spacing();
    ui.text_colored(theme::ACCENT, "ffmpeg is needed");
    ui.spacing();
    ui.text_wrapped(format!(
        "The packer uses ffmpeg to read and encode songs, and cannot find {} and {}. Install ffmpeg, then \
         point at the folder that holds it.",
        tools::executable("ffmpeg"),
        tools::executable("ffprobe")
    ));
    ui.spacing();
    ffmpeg_download_controls(app, ui, 300.0);
    ui.spacing();
    ui.text_disabled("- or install it yourself -");
    ui.spacing();
    if ui.button_with_size("Open the ffmpeg download page", [250.0, 32.0]) {
        let _ = opener::open(DOWNLOAD_PAGE);
    }
    ui.same_line();
    if ui.button_with_size("Locate ffmpeg...", [160.0, 32.0]) {
        app.locate_ffmpeg();
    }
}

/// The playlists shown as tabs: the default first, then custom ones, then any a song names, with
/// each one's song count and total length.
pub struct Playlists {
    pub names: Vec<String>,
    pub stats: Vec<(usize, f64)>,
}

impl Playlists {
    pub fn of(app: &App) -> Playlists {
        let mut playlists = Playlists { names: vec![app.default_playlist()], stats: vec![(0, 0.0)] };
        let add = |playlists: &mut Playlists, name: &str| -> usize {
            match playlists.names.iter().position(|known| known == name) {
                Some(index) => index,
                None => {
                    playlists.names.push(name.to_owned());
                    playlists.stats.push((0, 0.0));
                    playlists.names.len() - 1
                }
            }
        };
        for name in &app.custom_playlists {
            add(&mut playlists, name);
        }
        for row in &app.rows {
            let index = add(&mut playlists, &app.row_playlist(row));
            playlists.stats[index].0 += 1;
            if row.scanned {
                playlists.stats[index].1 += row.seconds;
            }
        }
        playlists
    }
}

fn songs_page(app: &mut App, ui: &Ui) {
    let playlists = Playlists::of(app);
    if app.filter.as_ref().is_some_and(|filter| !playlists.names.contains(filter)) {
        app.filter = None;
    }
    toolbar(app, ui, &playlists);
    ui.spacing();
    if app.rows.is_empty() {
        empty_state(app, ui);
    } else {
        song_table(app, ui, &playlists);
        footer(app, ui, &playlists);
    }
}

fn toolbar(app: &mut App, ui: &Ui, playlists: &Playlists) {
    let busy = app.busy();
    let (cover_label, cover_tip) = if playlists.names.len() > 1 {
        (
            format!("Covers ({})", playlists.names.len()),
            format!("Configure artwork for each of the {} playlists in this mod", playlists.names.len()),
        )
    } else if app.playlist.is_empty() {
        (
            "Cover: Auto".to_owned(),
            "Automatic cover: the first track's album art (or none). Name the playlist to customize.".to_owned(),
        )
    } else if let Some(image) = app.playlist_artwork.get(&app.playlist) {
        let name = file_name(image);
        let short =
            if name.chars().count() > 10 { format!("{}..", name.chars().take(8).collect::<String>()) } else { name };
        (format!("Cover: {short}"), format!("Custom cover image: {}", image.display()))
    } else if app.generated_artwork.contains(&app.playlist) {
        ("Cover: Text".to_owned(), "A generated text cover with the playlist name".to_owned())
    } else {
        (
            "Cover: Auto".to_owned(),
            "Automatic cover: the first track's album art (or none). Click to customize.".to_owned(),
        )
    };

    let _bg = ui.push_style_color(StyleColor::ChildBg, theme::CARD);
    let _border = ui.push_style_color(StyleColor::Border, theme::CARD_BORDER);
    let _padding = ui.push_style_var(StyleVar::WindowPadding([12.0, 8.0]));
    ui.child_window("toolbar")
        .size([0.0, 76.0])
        .child_flags(ChildFlags::BORDERS)
        .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
        .build(ui, || {
            let style = ui.clone_style();
            let pad = style.frame_padding()[0] * 2.0;
            let spacing = style.item_spacing()[0];
            let gap = 14.0;
            let button = |text: &str| ui.calc_text_size(text)[0] + pad;
            let normalize_width = ui.frame_height() + style.item_inner_spacing()[0] + ui.calc_text_size("Normalize")[0];
            let total = button("New Mod")
                + spacing
                + button("Open Mod...")
                + gap
                + 170.0
                + spacing
                + 160.0
                + spacing
                + button(&cover_label)
                + gap
                + 80.0
                + spacing
                + normalize_width
                + spacing
                + button("Settings...");
            center_next(ui, total);

            {
                let _disabled = ui.begin_disabled_with_cond(busy);
                if ui.button("New Mod") {
                    if app.has_unsaved_changes() {
                        app.dialogs.discard = Some(Pending::NewMod);
                    } else {
                        app.reset();
                    }
                }
                ui.same_line();
                if ui.button("Open Mod...") {
                    if app.has_unsaved_changes() {
                        app.dialogs.discard = Some(Pending::OpenMod);
                    } else {
                        app.open_mod_dialog();
                    }
                }
                ui.same_line_with_spacing(0.0, gap);
                ui.set_next_item_width(170.0);
                if ui.input_text("##name", &mut app.name).hint("Mod name").build() {
                    // A renamed new mod goes to its new folder; an opened one stays where it is.
                    if app
                        .output
                        .as_ref()
                        .is_some_and(|output| output.parent() == Some(&app.settings.game.join("Mods")))
                    {
                        app.output = None;
                    }
                }
                tooltip(ui, "Mod name (also the mod's folder name in Mods/)");
                ui.same_line();
                ui.set_next_item_width(160.0);
                ui.input_text("##playlist", &mut app.playlist).hint("Default playlist").build();
                tooltip(ui, "Default playlist name in skate. Songs use it unless the table gives them another.");
                ui.same_line();
                if ui.button(&cover_label) {
                    ui.open_popup("cover_popup");
                }
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        ui.text(&cover_tip);
                        if let Some(texture) = app.preview.texture {
                            ui.spacing();
                            ui.image(texture, [120.0, 120.0]);
                        }
                    });
                }
                dialogs::cover_popup(app, ui, playlists);
                ui.same_line_with_spacing(0.0, gap);
                ui.set_next_item_width(80.0);
                let labels: Vec<String> = BITRATES.iter().map(|rate| rate.to_string()).collect();
                ui.combo_simple_string("##bitrate", &mut app.bitrate, &labels);
                tooltip(ui, "Opus audio bitrate (kbps)");
                ui.same_line();
                ui.checkbox("Normalize", &mut app.normalize);
                tooltip(ui, "Equalise track loudness using EBU R128 (-15 LUFS)");
                ui.same_line();
                if ui.button("Settings...") {
                    app.dialogs.settings = true;
                }
            }
            ui.spacing();
            let folder = app.output_folder();
            let into = elide_middle(ui, &format!("Builds into: {}", folder.display()), ui.content_region_avail()[0]);
            centered_text(ui, &into, None);
            tooltip(ui, &folder.display().to_string());
        });
}

fn empty_state(app: &mut App, ui: &Ui) {
    let available = ui.content_region_avail();
    let width = (available[0] - 48.0).min(720.0);
    let height = (available[1] - 20.0).min(310.0);
    ui.set_cursor_pos_x(ui.cursor_pos_x() + ((available[0] - width) * 0.5).max(0.0));
    ui.set_cursor_pos_y(ui.cursor_pos()[1] + ((available[1] - height) * 0.5).max(0.0));

    let _bg = ui.push_style_color(StyleColor::ChildBg, [0.13, 0.15, 0.19, 0.90]);
    let _border = ui.push_style_color(StyleColor::Border, [0.25, 0.31, 0.42, 0.75]);
    let _rounding = ui.push_style_var(StyleVar::ChildRounding(14.0));
    let _border_size = ui.push_style_var(StyleVar::ChildBorderSize(1.5));
    let _padding = ui.push_style_var(StyleVar::WindowPadding([28.0, 20.0]));
    ui.child_window("empty")
        .size([width, height])
        .child_flags(ChildFlags::BORDERS)
        .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
        .build(ui, || {
            // An equaliser as the drop target's icon.
            let (icon_width, icon_height) = (72.0, 42.0);
            center_next(ui, icon_width);
            let origin = ui.cursor_screen_pos();
            ui.dummy([icon_width, icon_height]);
            let draw = ui.get_window_draw_list();
            let heights = [10.0, 18.0, 28.0, 38.0, 42.0, 38.0, 28.0, 18.0, 10.0f32];
            let (bar, bar_gap) = (4.5, 3.8);
            let start = origin[0] + (icon_width - (9.0 * bar + 8.0 * bar_gap)) * 0.5;
            let middle = origin[1] + icon_height * 0.5;
            for (index, height) in heights.iter().enumerate() {
                let x = start + index as f32 * (bar + bar_gap);
                let t = 1.0 - (index as f32 - 4.0).abs() / 5.0;
                let color = [0.18 + 0.14 * t, 0.52 + 0.28 * t, 0.92 + 0.08 * t, 0.70 + 0.30 * t];
                draw.add_rect([x, middle - height * 0.5], [x + bar, middle + height * 0.5], color)
                    .filled(true)
                    .rounding(2.5)
                    .build();
            }

            ui.dummy([0.0, 8.0]);
            centered_text(ui, "Drop Audio Files or Folders Here", Some([0.96, 0.97, 1.0, 1.0]));
            ui.spacing();
            centered_text(ui, "Drag and drop music straight onto this window, or browse below", None);
            centered_text(ui, "Supports MP3, FLAC, WAV, OGG, Opus, AAC, M4A, AIFF, and more", None);
            ui.dummy([0.0, 16.0]);

            center_next(ui, 165.0 + 14.0 + 145.0);
            {
                let _disabled = ui.begin_disabled_with_cond(app.busy());
                if colored_button(
                    ui,
                    "Browse Songs...",
                    [165.0, 34.0],
                    [[0.18, 0.48, 0.86, 0.95], [0.24, 0.56, 0.96, 1.0], [0.14, 0.40, 0.74, 1.0]],
                ) {
                    app.pick_songs(false);
                }
                ui.same_line_with_spacing(0.0, 14.0);
                if colored_button(ui, "Add Folder...", [145.0, 34.0], theme::SECONDARY) {
                    app.pick_songs(true);
                }
            }
            ui.dummy([0.0, 16.0]);
            centered_text(ui, "Tags and embedded album covers are read automatically.", None);
            centered_text(ui, "Playlists, track order and artwork can be changed once songs are added.", None);
        });
    if let Some((text, error)) = &app.status {
        ui.spacing();
        centered_text(ui, text, Some(if *error { theme::ERROR } else { theme::SUCCESS }));
    }
}

fn assign_menu(app: &mut App, ui: &Ui, playlists: &Playlists, index: usize) {
    let default = app.default_playlist();
    for name in &playlists.names {
        let current = app.row_playlist(&app.rows[index]) == *name;
        let label = if *name == default { format!("{name} (default)") } else { name.clone() };
        if ui.menu_item_enabled_selected_no_shortcut(&label, current, true) {
            app.rows[index].playlist = if *name == default { String::new() } else { name.clone() };
        }
    }
    ui.separator();
    if ui.menu_item("+ New Playlist...") {
        app.dialogs.new_playlist_row = Some(index);
        app.dialogs.new_playlist_name.clear();
        app.dialogs.new_playlist = true;
    }
}

enum RowAction {
    Swap(usize, usize),
    Remove(usize),
}

fn song_table(app: &mut App, ui: &Ui, playlists: &Playlists) {
    let busy = app.busy();
    let problems: Vec<Vec<String>> = (0..app.rows.len()).map(|index| app.row_problems(index)).collect();

    // Playlist tabs, when there is more than one playlist or a custom one.
    if playlists.names.len() > 1 || !app.custom_playlists.is_empty() {
        if let Some(_bar) =
            ui.tab_bar_with_flags("playlist_tabs", TabBarOptions::new().fitting_policy(TabBarFittingPolicy::Scroll))
        {
            let select = app.select_tab.take();
            let all_flags = if select == Some(None) { TabItemFlags::SET_SELECTED } else { TabItemFlags::empty() };
            if ui.tab_item_with_flags(format!("All ({})###tab_all", app.rows.len()), None, all_flags).is_some() {
                app.filter = None;
            }
            let default = app.default_playlist();
            for (name, &(count, _)) in playlists.names.iter().zip(&playlists.stats) {
                let flags = if select.as_ref() == Some(&Some(name.clone())) {
                    TabItemFlags::SET_SELECTED
                } else {
                    TabItemFlags::empty()
                };
                let mut open = true;
                let closable = *name != default && count == 0;
                let label = format!("{name} ({count})###tab_{name}");
                if ui.tab_item_with_flags(label, closable.then_some(&mut open), flags).is_some() {
                    app.filter = Some(name.clone());
                }
                if closable && !open {
                    app.custom_playlists.retain(|custom| custom != name);
                    if app.filter.as_ref() == Some(name) {
                        app.filter = None;
                    }
                }
            }
            let plus = TabItemOptions::new().flags(TabItemFlags::NO_TOOLTIP).placement(TabItemPlacement::Trailing);
            if ui.tab_item_button_with_flags("+", plus) {
                app.dialogs.new_playlist = true;
                app.dialogs.new_playlist_name.clear();
                app.dialogs.new_playlist_row = None;
            }
            tooltip(ui, "Create a new playlist");
        }
        ui.spacing();
    }

    let footer = ui.frame_height_with_spacing() * 3.5 + 16.0;
    let flags = TableFlags::ROW_BG
        | TableFlags::BORDERS_OUTER
        | TableFlags::BORDERS_INNER_H
        | TableFlags::SCROLL_Y
        | TableFlags::RESIZABLE;
    let mut action = None;
    if let Some(_table) =
        ui.begin_table_with_sizing("songs", 7, flags, [0.0, ui.content_region_avail()[1] - footer], 0.0)
    {
        ui.table_setup_scroll_freeze(0, 1);
        let fixed = |width| Some(TableColumnWidth::Fixed(width));
        let stretch = |weight| Some(TableColumnWidth::Stretch(weight));
        ui.table_setup_column("#", TableColumnFlags::empty(), fixed(32.0));
        ui.table_setup_column("Artist", TableColumnFlags::empty(), stretch(1.1));
        ui.table_setup_column("Title", TableColumnFlags::empty(), stretch(1.3));
        ui.table_setup_column("Playlist (?)", TableColumnFlags::empty(), stretch(1.0));
        ui.table_setup_column("Length", TableColumnFlags::empty(), fixed(64.0));
        ui.table_setup_column("Problems", TableColumnFlags::empty(), stretch(0.9));
        ui.table_setup_column("Actions", TableColumnFlags::empty(), fixed(196.0));
        ui.table_next_row_with_flags(TableRowFlags::HEADERS, 0.0);
        for (column, name) in
            ["#", "Artist", "Title", "Playlist (?)", "Length", "Problems", "Actions"].iter().enumerate()
        {
            if !ui.table_set_column_index(column) {
                continue;
            }
            let _id = ui.push_id(column as i32);
            ui.table_header(name);
            if column == 3 {
                tooltip(
                    ui,
                    "The song's playlist.\nLeave blank for the default playlist, or pick or type another name to group songs into separate playlists.",
                );
            }
        }

        let visible: Vec<usize> = (0..app.rows.len())
            .filter(|&index| app.filter.as_ref().is_none_or(|filter| app.row_playlist(&app.rows[index]) == *filter))
            .collect();
        for (position, &index) in visible.iter().enumerate() {
            let previous = position.checked_sub(1).map(|p| visible[p]);
            let next = visible.get(position + 1).copied();
            let _id = ui.push_id(index as i32);
            ui.table_next_row();

            ui.table_next_column();
            ui.selectable_config((index + 1).to_string())
                .flags(SelectableFlags::SPAN_ALL_COLUMNS | SelectableFlags::ALLOW_OVERLAP)
                .build();
            tooltip(ui, &format!("{}\n(Right-click the row for options)", app.rows[index].file.display()));
            if app.dialogs.context_row == Some(index) {
                ui.open_popup("row_context");
                app.dialogs.context_row = None;
            }
            if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("row_context")) {
                ui.text_disabled(app.rows[index].id());
                ui.separator();
                if let Some(_menu) = ui.begin_menu("Assign to Playlist") {
                    assign_menu(app, ui, playlists, index);
                }
                if let Some(_menu) = ui.begin_menu("Artwork") {
                    if ui.menu_item("Choose Image...")
                        && let Some(image) = App::pick_image()
                    {
                        app.rows[index].artwork = Some(image);
                    }
                    if app.rows[index].artwork.is_some() && ui.menu_item("Use Embedded / Automatic Artwork") {
                        app.rows[index].artwork = None;
                    }
                    if ui.menu_item("Manage All Artwork...") {
                        app.dialogs.artwork = true;
                    }
                }
                ui.separator();
                if ui.menu_item_enabled_selected_no_shortcut("Move Up", false, previous.is_some() && !busy) {
                    action = previous.map(|previous| RowAction::Swap(index, previous));
                }
                if ui.menu_item_enabled_selected_no_shortcut("Move Down", false, next.is_some() && !busy) {
                    action = next.map(|next| RowAction::Swap(index, next));
                }
                ui.separator();
                if ui.menu_item_enabled_selected_no_shortcut("Remove Song", false, !busy) {
                    action = Some(RowAction::Remove(index));
                }
            }

            {
                let _disabled = ui.begin_disabled_with_cond(busy);
                ui.table_next_column();
                ui.set_next_item_width(-1.0);
                ui.input_text("##artist", &mut app.rows[index].artist).build();
                ui.table_next_column();
                ui.set_next_item_width(-1.0);
                ui.input_text("##title", &mut app.rows[index].title).build();

                ui.table_next_column();
                let arrow = ui.frame_height();
                let spacing = ui.clone_style().item_spacing()[0];
                ui.set_next_item_width((ui.content_region_avail()[0] - arrow - spacing).max(30.0));
                let default = app.default_playlist();
                ui.input_text("##playlist", &mut app.rows[index].playlist).hint(default.clone()).build();
                tooltip(
                    ui,
                    &format!("The song's playlist.\nLeave blank for \"{default}\", or pick or type another name."),
                );
                ui.same_line_with_spacing(0.0, spacing);
                if ui.arrow_button("##playlist_menu", Direction::Down) {
                    ui.open_popup("playlist_menu");
                }
                tooltip(ui, "Choose an existing playlist or create one");
                if let Some(_popup) = ui.begin_popup("playlist_menu") {
                    ui.text_disabled("Assign to Playlist");
                    ui.separator();
                    assign_menu(app, ui, playlists, index);
                }
            }

            ui.table_next_column();
            if app.rows[index].scanned {
                ui.text(duration(app.rows[index].seconds));
            } else {
                ui.text_disabled("...");
            }

            ui.table_next_column();
            if !problems[index].is_empty() {
                ui.text_colored(theme::PROBLEM, problems[index].join("; "));
            }

            ui.table_next_column();
            let _disabled = ui.begin_disabled_with_cond(busy);
            {
                let _no_previous = ui.begin_disabled_with_cond(previous.is_none());
                if ui.arrow_button("##up", Direction::Up)
                    && let Some(previous) = previous
                {
                    action = Some(RowAction::Swap(index, previous));
                }
                tooltip(ui, "Move song up");
            }
            ui.same_line();
            {
                let _no_next = ui.begin_disabled_with_cond(next.is_none());
                if ui.arrow_button("##down", Direction::Down)
                    && let Some(next) = next
                {
                    action = Some(RowAction::Swap(index, next));
                }
                tooltip(ui, "Move song down");
            }
            ui.same_line();
            if colored_button(ui, "Remove", [0.0, 0.0], theme::DANGER) {
                action = Some(RowAction::Remove(index));
            }
            tooltip(ui, "Remove this song");
            ui.same_line();
            if ui.button("...##more") {
                app.dialogs.context_row = Some(index);
            }
            tooltip(ui, "More options (or right-click the row)");
        }

        if visible.is_empty()
            && let Some(filter) = &app.filter
        {
            ui.table_next_row();
            ui.table_set_column_index(1);
            ui.dummy([0.0, 8.0]);
            ui.text_disabled(format!("No songs in \"{filter}\" yet."));
            ui.text_disabled("Drop audio files here or click 'Add songs...' below to add tracks to this playlist.");
            ui.dummy([0.0, 8.0]);
        }

        let flags = PopupContextOptions::new().flags(PopupContextFlags::NO_OPEN_OVER_ITEMS);
        if let Some(_popup) = ui.begin_popup_context_window_with_flags(Some("table_context"), flags) {
            if ui.menu_item("Add Songs...") {
                app.pick_songs(false);
            }
            if ui.menu_item("Add Folder...") {
                app.pick_songs(true);
            }
            ui.separator();
            if ui.menu_item("+ New Playlist...") {
                app.dialogs.new_playlist_row = None;
                app.dialogs.new_playlist_name.clear();
                app.dialogs.new_playlist = true;
            }
            if ui.menu_item("Manage Artwork...") {
                app.dialogs.artwork = true;
            }
        }
    }
    match action {
        Some(RowAction::Swap(from, to)) => app.move_row(from, to),
        Some(RowAction::Remove(index)) => {
            app.rows.remove(index);
        }
        None => {}
    }
}

fn footer(app: &mut App, ui: &Ui, playlists: &Playlists) {
    let busy = app.busy();
    ui.spacing();
    {
        let _disabled = ui.begin_disabled_with_cond(busy);
        let target = app.filter.clone();
        if ui.button_with_size("Add songs...", [105.0, 26.0]) {
            app.pick_songs(false);
        }
        if let Some(target) = &target {
            tooltip(ui, &format!("Add songs straight into \"{target}\""));
        }
        ui.same_line();
        if ui.button_with_size("Add folder...", [105.0, 26.0]) {
            app.pick_songs(true);
        }
        if let Some(target) = &target {
            tooltip(ui, &format!("Add a folder's songs straight into \"{target}\""));
        }
    }

    ui.same_line();
    let all_scanned = app.rows.iter().all(|row| row.scanned);
    let total: f64 = app.rows.iter().filter(|row| row.scanned).map(|row| row.seconds).sum();
    let mut summary = String::new();
    if playlists.names.len() > 1 {
        summary.push_str(&format!("{} playlists | ", playlists.names.len()));
    }
    summary.push_str(&format!("{} song{}", app.rows.len(), if app.rows.len() == 1 { "" } else { "s" }));
    summary.push_str(&if all_scanned { format!(" ({})", duration(total)) } else { " (scanning tags...)".into() });
    let width = ui.calc_text_size(&summary)[0];
    let available = ui.content_region_avail()[0];
    if available > width {
        ui.set_cursor_pos_x(ui.cursor_pos_x() + available - width);
    }
    ui.text_disabled(&summary);
    if playlists.names.len() > 1 && ui.is_item_hovered() {
        ui.tooltip(|| {
            ui.text("Playlist Breakdown");
            ui.separator();
            for (name, &(count, seconds)) in playlists.names.iter().zip(&playlists.stats) {
                ui.bullet_text(format!(
                    "{name}: {count} song{} ({})",
                    if count == 1 { "" } else { "s" },
                    duration(seconds)
                ));
            }
        });
    }

    let blocker = app.build_blocker();
    if busy {
        let label = format!("{}: {}", app.jobs.label().unwrap_or_default(), app.jobs.text);
        ui.progress_bar(app.jobs.fraction).size([-120.0, 28.0]).overlay_text(label).build();
        ui.same_line();
        if ui.button_with_size("Cancel", [-1.0, 28.0]) {
            app.jobs.cancel();
        }
        return;
    }
    if let Some((text, error)) = &app.status {
        ui.text_colored(if *error { theme::ERROR } else { theme::SUCCESS }, text);
    } else if let Some(blocker) = &blocker {
        ui.text_disabled(blocker);
    } else if app.game_running.load(Ordering::Relaxed) {
        ui.text_colored(theme::WARNING, "skate. is running: the mod takes effect the next time it starts.");
    } else {
        ui.text("");
    }
    let _disabled = ui.begin_disabled_with_cond(blocker.is_some());
    let available = ui.content_region_avail()[0];
    let width = if blocker.is_none() { (available - 10.0) * 0.65 } else { available };
    let mods = std::path::Path::new("Mods").join(file_name(&app.output_folder()));
    if colored_button(ui, &format!("Build into {}", mods.display()), [width, 32.0], theme::PRIMARY) {
        app.request_build();
    }
    if blocker.is_none() {
        ui.same_line();
        if ui.button_with_size("Export Thunderstore...", [-1.0, 32.0]) {
            app.dialogs.export = true;
            if app.export.description.is_empty() {
                app.export.description = format!("Adds {} song(s) to skate.", app.rows.len());
            }
        }
    }
}
