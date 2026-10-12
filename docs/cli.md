# CLI reference

```text
ReSkateMusicPacker <game folder> <song folder> [output folder] [options]
```

- **game folder** - the ReSkate install containing `Skate.exe`.
- **song folder** - a folder of audio files. The CLI reads the files directly in that folder (it does
  not recurse), ignoring images.
- **output folder** - where the mod is written. Defaults to `<song folder>_mod` next to the song
  folder.

Run with no arguments for the window, or `--gui [songs or folders...]` to open it with songs added.

## Options

| Flag | Description | Default |
|---|---|---|
| `--name <mod name>` | Display name of the mod. | Song folder name |
| `--playlist <playlist>` | Playlist name shown in the in-game music menu. | Song folder name |
| `--bitrate <kbps>` | Opus bitrate, 64-320. | `192` |
| `--no-normalize` | Disable EBU R128 loudness normalisation (two-pass to -15 LUFS). | Normalised |
| `--playlist-artwork <name> <image>` | Cover image for the named playlist. | None |
| `--track-artwork <id> <image>` | Cover for the exact `Artist - Title` song id; repeat for several. | None |
| `--generate-playlist-artwork <name>` | Generate a text cover from the playlist name. | Off |
| `--thunderstore` | Also export a Thunderstore-ready `.zip` (see below). | Off |
| `--author <name>` | Author namespace in the Thunderstore manifest. | `Author` |
| `--version <x.y.z>` | Thunderstore package version. | `1.0.0` |
| `--no-readme-credit` | Omit the packer credit link from the generated README. | Credit included |
| `--ffmpeg <folder>` | Folder holding ffmpeg and ffprobe. | The window's setting, then `PATH` |
| `--get-ffmpeg [folder]` | Download the pinned static ffmpeg build, print where it went, then exit. | Beside the program or the local app data folder |
| `--gui` | Open the window. | |
| `--help`, `-h` | Show usage. | |

Any image ffmpeg can read (PNG, JPEG, WebP, BMP, ...) can be used for artwork. Covers are scaled
proportionally to a 512x512 PNG with dark padding, saved inside the mod.

Exit status: `0` on success, `1` for a usage error, `2` when packing fails.

## Examples

```powershell
ReSkateMusicPacker.exe "F:\Games\ReSkate-1.0.0" "C:\Music\My Mix" --playlist "My Mix"

ReSkateMusicPacker.exe "F:\Games\ReSkate-1.0.0" "C:\Music\My Mix" "C:\Mods\My Mix" `
    --name "My Mix" --playlist "My Mix" --bitrate 224 `
    --playlist-artwork "My Mix" "C:\Pictures\mix.png" `
    --track-artwork "MGMT - Kids" "C:\Pictures\kids.png" `
    --thunderstore --author MyName --version 1.0.0
```

```sh
# macOS, game under CrossOver/Whisky
ReSkateMusicPacker ~/Games/ReSkate ~/Music/"My Mix" --generate-playlist-artwork "My Mix"
```

## ffmpeg download

`--get-ffmpeg` downloads the pinned static LGPL Windows build (BtbN/FFmpeg-Builds) the window's
button uses, verifies its SHA-256, extracts `ffmpeg.exe` + `ffprobe.exe` and prints the folder.
`FFMPEG_URL` and `FFMPEG_SHA256` override the pinned source (and are required on macOS and Linux,
where there is no pinned build).

## Caching

Encoded Opus is cached by source-file hash and settings in the user cache folder
(`%LOCALAPPDATA%\ReSkateMusicPacker\cache\` on Windows), so rebuilding only re-encodes what changed.

## Thunderstore

`--thunderstore` writes `<Author>-<Name>-<Version>.zip` beside the mod. It contains the whole mod
(minus its local project file) plus a Thunderstore `manifest.json`, a generated `README.md` tracklist,
and a 256x256 `icon.png` (the mod's own `icon.png` if it has one, otherwise generated).
