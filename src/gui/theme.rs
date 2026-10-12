// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The window's look: a dark slate theme with one blue accent.

use dear_imgui_rs::{Style, StyleColor as C};

pub const ACCENT: [f32; 4] = [0.22, 0.65, 1.0, 1.0];
pub const ERROR: [f32; 4] = [1.0, 0.45, 0.40, 1.0];
pub const SUCCESS: [f32; 4] = [0.40, 0.85, 0.50, 1.0];
pub const WARNING: [f32; 4] = [1.0, 0.80, 0.35, 1.0];
pub const PROBLEM: [f32; 4] = [1.0, 0.55, 0.35, 1.0];

/// Button colours (normal, hovered, active).
pub const PRIMARY: [[f32; 4]; 3] = [[0.16, 0.45, 0.78, 1.0], [0.22, 0.54, 0.90, 1.0], [0.13, 0.38, 0.68, 1.0]];
pub const DANGER: [[f32; 4]; 3] = [[0.48, 0.16, 0.16, 0.85], [0.68, 0.22, 0.22, 1.0], [0.78, 0.15, 0.15, 1.0]];
pub const SECONDARY: [[f32; 4]; 3] = [[0.20, 0.24, 0.30, 0.95], [0.27, 0.32, 0.40, 1.0], [0.16, 0.20, 0.25, 1.0]];

pub const CARD: [f32; 4] = [0.13, 0.14, 0.18, 0.95];
pub const CARD_BORDER: [f32; 4] = [0.22, 0.26, 0.33, 0.80];

pub fn apply(style: &mut Style) {
    style.set_window_padding([16.0, 14.0]);
    style.set_frame_padding([9.0, 6.0]);
    style.set_cell_padding([8.0, 6.0]);
    style.set_item_spacing([8.0, 8.0]);
    style.set_item_inner_spacing([6.0, 6.0]);
    style.set_scrollbar_size(13.0);
    style.set_grab_min_size(10.0);
    style.set_window_rounding(0.0);
    style.set_child_rounding(8.0);
    style.set_frame_rounding(6.0);
    style.set_popup_rounding(8.0);
    style.set_scrollbar_rounding(6.0);
    style.set_grab_rounding(4.0);
    style.set_tab_rounding(6.0);
    style.set_window_border_size(0.0);
    style.set_frame_border_size(1.0);
    style.set_popup_border_size(1.0);

    let colors: &[(C, [f32; 4])] = &[
        (C::WindowBg, [0.11, 0.12, 0.14, 1.0]),
        (C::ChildBg, [0.12, 0.13, 0.16, 1.0]),
        (C::PopupBg, [0.13, 0.14, 0.17, 0.98]),
        (C::Border, [0.22, 0.24, 0.28, 0.70]),
        (C::BorderShadow, [0.0, 0.0, 0.0, 0.0]),
        (C::Text, [0.92, 0.93, 0.95, 1.0]),
        (C::TextDisabled, [0.50, 0.54, 0.58, 1.0]),
        (C::FrameBg, [0.16, 0.18, 0.22, 1.0]),
        (C::FrameBgHovered, [0.21, 0.24, 0.30, 1.0]),
        (C::FrameBgActive, [0.25, 0.29, 0.36, 1.0]),
        (C::TitleBg, [0.09, 0.10, 0.12, 1.0]),
        (C::TitleBgActive, [0.11, 0.12, 0.15, 1.0]),
        (C::TitleBgCollapsed, [0.09, 0.10, 0.12, 0.75]),
        (C::MenuBarBg, [0.13, 0.14, 0.17, 1.0]),
        (C::ScrollbarBg, [0.10, 0.11, 0.13, 0.50]),
        (C::ScrollbarGrab, [0.26, 0.29, 0.35, 0.80]),
        (C::ScrollbarGrabHovered, [0.34, 0.38, 0.46, 0.90]),
        (C::ScrollbarGrabActive, [0.42, 0.47, 0.56, 1.0]),
        (C::CheckMark, ACCENT),
        (C::SliderGrab, [0.22, 0.58, 0.95, 0.90]),
        (C::SliderGrabActive, [0.30, 0.68, 1.0, 1.0]),
        (C::Button, [0.19, 0.22, 0.27, 1.0]),
        (C::ButtonHovered, [0.26, 0.31, 0.39, 1.0]),
        (C::ButtonActive, [0.16, 0.20, 0.25, 1.0]),
        (C::Header, [0.18, 0.21, 0.26, 1.0]),
        (C::HeaderHovered, [0.24, 0.28, 0.35, 1.0]),
        (C::HeaderActive, [0.28, 0.34, 0.42, 1.0]),
        (C::Separator, [0.22, 0.24, 0.28, 0.80]),
        (C::SeparatorHovered, [0.28, 0.32, 0.38, 0.90]),
        (C::SeparatorActive, [0.35, 0.42, 0.52, 1.0]),
        (C::ResizeGrip, [0.22, 0.58, 0.95, 0.25]),
        (C::ResizeGripHovered, [0.22, 0.58, 0.95, 0.67]),
        (C::ResizeGripActive, [0.22, 0.58, 0.95, 0.95]),
        (C::TableHeaderBg, [0.15, 0.17, 0.21, 1.0]),
        (C::TableBorderStrong, [0.22, 0.25, 0.30, 1.0]),
        (C::TableBorderLight, [0.18, 0.20, 0.24, 0.70]),
        (C::TableRowBg, [0.12, 0.13, 0.16, 0.70]),
        (C::TableRowBgAlt, [0.14, 0.15, 0.18, 0.70]),
        (C::Tab, [0.15, 0.17, 0.21, 1.0]),
        (C::TabHovered, [0.24, 0.28, 0.35, 1.0]),
        (C::TabSelected, [0.19, 0.23, 0.29, 1.0]),
        (C::TabSelectedOverline, ACCENT),
        (C::TabDimmed, [0.13, 0.14, 0.17, 1.0]),
        (C::TabDimmedSelected, [0.16, 0.18, 0.23, 1.0]),
        (C::PlotHistogram, [0.18, 0.52, 0.92, 1.0]),
        (C::PlotHistogramHovered, [0.25, 0.60, 1.0, 1.0]),
        (C::ModalWindowDimBg, [0.05, 0.05, 0.07, 0.65]),
    ];
    for &(color, value) in colors {
        style.set_color(color, value);
    }
}
