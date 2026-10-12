# Troubleshooting

## ffmpeg / ffprobe not found

The packer needs both `ffmpeg` and `ffprobe` (`.exe` on Windows). Put them on your `PATH`, point the
window at their folder (**Locate ffmpeg...**), or pass `--ffmpeg <folder>` on the command line. A full
ffmpeg build includes both. On macOS: `brew install ffmpeg`; on Linux, your package manager's ffmpeg.

If you'd rather not install it yourself, the **Download ffmpeg automatically** button (first-run setup
or **Settings...**) fetches a pinned static LGPL build from
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) into `.\ffmpeg` beside the app when that
folder is writable, otherwise `%LOCALAPPDATA%\ReSkateMusicPacker\ffmpeg`. The pinned build is for
Windows; elsewhere the button explains how to install ffmpeg instead (or set `FFMPEG_URL` and
`FFMPEG_SHA256`).

## The automatic ffmpeg download fails

- **Offline / blocked / a corporate proxy**: on Windows the download uses the system's TLS and certificates, but
  needs to reach `github.com` and its asset host. Elsewhere it honours `HTTPS_PROXY`. Point the tool at a local ffmpeg folder instead
  (**Locate ffmpeg...**), or set `FFMPEG_URL` (and `FFMPEG_SHA256`) to a mirror.
- **HTTP 404**: the pinned release was pruned. Set `FFMPEG_URL` to a current asset and `FFMPEG_SHA256`
  to its SHA-256, or use **Locate ffmpeg...**. Release builds also re-check the pinned link before
  packaging, so this is normally caught before a release ships.
- **"did not match its expected checksum"**: the download was corrupted or the pinned asset changed.
  Retry; if it persists, update `FFMPEG_URL`/`FFMPEG_SHA256`.
- **SmartScreen / antivirus** may flag a freshly downloaded `ffmpeg.exe` because it is new and
  unsigned. That is expected for a first-run download; allow it, or install ffmpeg yourself.
- The download is about **163 MB** and is only started when you click, never on startup.

## Song clash warning

Every song id is the string `Artist - Title`. If a song's id already exists in another installed mod
or in the game's soundtrack, the mod can't add it (and a duplicate id can break the game's music
catalog). Rename the artist/title (or fix the file's tags) so the id is unique, then rebuild.

## Cover art doesn't show

- The runtime must support mod artwork. On an older ReSkate runtime the covers are ignored (music still
  plays).
- Check `reskate-music.json` references the file by a path relative to the mod folder, `.png`, with no
  `..` or `\`.
- The runtime only serves artwork for songs it can resolve by their `Artist - Title` id, so make sure
  the id in `song_artwork` matches the song exactly.

## Mod tracks sound quiet (or loud) next to the game's music

The packer normalises to **-15 LUFS** (true peak -1.5 dBTP) with two-pass `loudnorm`, the level the
game's own tracks sit at. If you disabled normalisation, or if you're comparing against a differently
mastered source, this will differ. Use `--no-normalize` to pack the source level unchanged.

## "assets are stored uncompressed"

Outside Windows the game's Oodle encoder is not available, so the few small EBX assets a mod adds
are stored as raw CAS blocks, a form the game reads natively. Audio was always stored raw. Building
on Windows compresses them with the game's own codec, exactly as the C++ tool did.

## Thunderstore export

The `.zip` is written beside the mod (or to the output you gave). Give it an author and version with
`--author` / `--version` (or the export dialog). It contains the full mod plus `manifest.json`, a
generated `README.md` tracklist and a 256x256 `icon.png`.

## The game doesn't see the mod

- Make sure the mod folder is in your ReSkate `Mods` folder and enabled in the launcher.
- ReSkate merges mods at launch; restart the game (or re-run the launcher) after adding a mod.
- The `.reskate-studio-patch` records the game build the mod targets. A mod built for a different game
  build may be left out by the merger; rebuild against the game folder you're playing.

## Where the files live

- Settings: `%APPDATA%\ReSkateMusicPacker\settings.json` (macOS: `~/Library/Application Support/ReSkateMusicPacker/`,
  Linux: `~/.config/ReSkateMusicPacker/`)
- Encode cache: `%LOCALAPPDATA%\ReSkateMusicPacker\cache\` (macOS: `~/Library/Caches/ReSkateMusicPacker/cache/`,
  Linux: `~/.cache/ReSkateMusicPacker/cache/`); delete it to force a full re-encode
- Downloaded ffmpeg: `.\ffmpeg` beside the app (portable) or `%LOCALAPPDATA%\ReSkateMusicPacker\ffmpeg`
