# Changelog

All notable changes to ReSkate Music Packer are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.0.0] - Unreleased

A rewrite in Rust. Same Dear ImGui interface, command line and mod format; mods and projects made
by 1.x open and rebuild unchanged.

### Added

- **macOS and Linux** builds. Oodle-compressed game data is decoded with an open-source decoder, so
  the packer works with a game installed under Wine/CrossOver.
- `--ffmpeg <folder>` on the command line; the window's saved ffmpeg folder is also used.
- `--gui <songs or folders>` opens the window with songs already added (dropping files on the
  program's icon does the same).
- Generated covers fall back to system CJK fonts for characters the bundled font lacks.
- The window can be resized; long paths are shortened in the middle to fit.
- ffmpeg failures include ffmpeg's own error lines.
- An end-to-end test packs real songs against a synthetic game and reads the mod back.

### Changed

- Dear ImGui draws through winit + OpenGL instead of Win32 + Direct3D 12.
- Generated covers use the bundled Inter font instead of Windows GDI, so they look the same on
  every system.
- Thunderstore export runs in the background, zips straight from the mod folder in a reproducible
  order, and never leaves a partial zip.
- `--name` defaults to the song folder's name, as documented (1.x used "ReSkateMusic").
- Package directories are matched case-insensitively, for case-sensitive file systems.
- On Windows the ffmpeg download uses the system TLS stack and certificate store.

### Fixed

- Generated covers no longer break a line one word early when the line ends exactly at a word.

### Removed

- Engine modules only ReSkate itself used (bundle-reference tables, material grids, shader lookups,
  EBX merging).
