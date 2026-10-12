// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Embedded fonts: Inter (SIL Open Font License 1.1, see assets/fonts/Inter-LICENSE.txt), used by
//! the window and by generated playlist covers so they look the same on every system.

use std::path::PathBuf;

pub const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
pub const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
pub const INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");

/// System fonts that cover scripts Inter lacks (CJK, Hangul, ...), most specific first, each with
/// the index of its bold face in a collection. Text falls back to them glyph by glyph; any that
/// are missing are skipped.
pub fn fallback_fonts() -> Vec<(PathBuf, u32)> {
    let candidates: Vec<(PathBuf, u32)> = if cfg!(windows) {
        let fonts = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
        [("YuGothB.ttc", 0), ("msyhbd.ttc", 0), ("malgunbd.ttf", 0), ("seguisym.ttf", 0)]
            .into_iter()
            .map(|(name, index)| (fonts.join(name), index))
            .collect()
    } else if cfg!(target_os = "macos") {
        [
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", 1),
            ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
            ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
        ]
        .into_iter()
        .map(|(path, index)| (PathBuf::from(path), index))
        .collect()
    } else {
        [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Bold.ttc",
        ]
        .into_iter()
        .map(|path| (PathBuf::from(path), 0))
        .collect()
    };
    candidates.into_iter().filter(|(path, _)| path.is_file()).collect()
}
