# ReSkate Music Packer

Turns a folder of songs into an **add-only music mod** for *skate.* running on
[ReSkate](https://github.com/Dingo-Shenanigans/ReSkate): every song gets its own assets and audio,
nothing the game ships is replaced, and the songs show up in the in-game music menu under your own
playlists, with cover art.

This is the Rust rewrite of the original C++ tool. It keeps the same Dear ImGui interface, the same
command line and the same mod format, and now runs on Windows, macOS and Linux.

## Using it

Run `ReSkateMusicPacker` with no arguments for the window:

1. Pick the game folder (the one with `Skate.exe`).
2. Let it find ffmpeg. On Windows it can download a pinned, checksummed build for you; on macOS
   use `brew install ffmpeg`; on Linux use your package manager.
3. Drop songs or folders onto the window, fix any artist/title it flags, name the mod and its
   playlist, and click **Build**. The mod lands in the game's `Mods` folder.

A built mod can be reopened with **Open Mod...** and rebuilt, and exported as a Thunderstore
package.

From a terminal:

```text
ReSkateMusicPacker <game folder> <song folder> [output folder] [options]
ReSkateMusicPacker --help
```

See [docs/cli.md](docs/cli.md) for every option. `ReSkateMusicPacker --gui <songs or folders>`
opens the window with those songs already added.

## Building

Needs Rust 1.88 or newer and a C++ compiler (Dear ImGui is compiled from source).

```text
cargo build --release              # the window and the command line
cargo build --release --no-default-features   # command line only, no GUI dependencies
cargo test                         # unit tests + an end-to-end pack against a synthetic game
```

The end-to-end test needs `ffmpeg` and `ffprobe` on `PATH` and skips itself without them.

## What changed from the C++ version

- **Cross-platform.** Oodle-compressed game data is decoded in pure Rust (`oozextract`), so the
  packer also works on macOS and Linux with the game installed under Wine/CrossOver. On Windows the
  game's own `oo2core` DLL is still used, so output there matches the C++ tool. Elsewhere the small
  EBX assets are stored uncompressed, which the game reads too; the CLI prints a note when it does.
- **Renderer.** Dear ImGui now draws through winit + OpenGL instead of Win32 + Direct3D 12, and the
  window can be resized.
- **Generated covers** are drawn with the bundled Inter font instead of Windows GDI, with system
  CJK fonts as a fallback, so they look the same everywhere. A wrapping quirk that broke lines one
  word early is fixed.
- **Thunderstore export** zips straight from the mod folder (no staging copy), writes a reproducible
  file order, runs in the background, and never leaves a half-written zip.
- **ffmpeg errors** now include ffmpeg's own last lines of output.
- **CLI:** `--name` really defaults to the song folder's name (as the docs always said), and
  `--ffmpeg <folder>` points at an ffmpeg install; the window's saved ffmpeg folder is used otherwise.
- **Dropped:** engine modules that only ReSkate itself used (bundle-reference tables, material
  grids, shader lookups, EBX merging).

Settings, the encode cache and the mod format are where they were (`%APPDATA%` and
`%LOCALAPPDATA%\ReSkateMusicPacker` on Windows), so existing caches and projects carry over.

## License

GPL-3.0-only; see [NOTICE.md](NOTICE.md) for attribution and bundled assets.
