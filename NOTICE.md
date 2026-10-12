# Notices and attribution

ReSkateMusicPacker is distributed under the [GNU General Public License, version 3](LICENSE)
(GPL-3.0). Copyright © 2026 DeckardDetribine and the ReSkateMusicPacker contributors.

## ReSkate

The engine code under `src/engine/` (Frostbite bundle, TOC, CAS, EBX and native DB handling) is a
Rust port of the engine of the **ReSkate** project (<https://github.com/Dingo-Shenanigans/ReSkate>),
Copyright © 2026 the ReSkate contributors, which is licensed under GPL-3.0. The source comments in
those files also credit the upstream *ReSkateStudio* `sdk/` they were originally ported from.

Because that code is GPL-3.0, ReSkateMusicPacker as a whole is distributed under GPL-3.0. If you
share a modified version, share its source code under the same license.

## Bundled assets

| Asset | Path | License |
|---|---|---|
| Inter typeface (Regular, SemiBold, Bold) | `assets/fonts` | SIL Open Font License 1.1 (`assets/fonts/Inter-LICENSE.txt`) |

## Third-party libraries

The Rust crates this program is built from keep their own licenses (MIT, Apache-2.0, BSD, zlib and
similar permissive licenses); `cargo tree` lists them. The notable ones:

| Library | Use | License |
|---|---|---|
| Dear ImGui (via `dear-imgui-rs`, `dear-imgui-winit`, `dear-imgui-glow`) | the window | MIT (bindings MIT / Apache-2.0) |
| winit, glutin, glow | windowing and OpenGL | Apache-2.0 / MIT |
| oozextract | Oodle (Kraken, Selkie, Leviathan) decoding | MIT |
| ruzstd, lz4_flex, miniz_oxide | Zstandard, LZ4 and zlib decoding | MIT / Apache-2.0 / zlib |
| ab_glyph, png | generated covers | Apache-2.0 / MIT |
| zip, ureq, rfd, sysinfo | packages, downloads, file dialogs, process check | MIT / Apache-2.0 |

ffmpeg is not bundled. The optional download fetches a static LGPL build from BtbN/FFmpeg-Builds,
which carries its own license.

## Game content

*skate.* and its content belong to Electronic Arts. Nothing in this repository grants any rights to
them.
