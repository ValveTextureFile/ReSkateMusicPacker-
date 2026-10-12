# The mod format

A ReSkate music mod is **add-only**: it adds new songs (and optionally a playlist) to the game without
replacing any shipped file. This is what the packer writes and what ReSkate reads.

## Contents of a built mod

```
My Mix/
  layout.toc                                  # the mod's TOC (declares its bundles)
  Win32/
    configurations/layout/defaultinstallpackage/cas_01.cas   # the mod's archive
    levels/game/bam_levelroot/bam_levelroot.toc              # its bundle manifest + chunks
  manifest.json                  # mod name/version/description
  .reskate-studio-patch          # the game build this mod targets (Skate.exe sha256)
  reskate-build.json             # tool + schema marker
  reskate-music.json             # playlists and cover art (read by ReSkate)
  reskate-music-project.json     # the source project, so the mod can be reopened/rebuilt
  artwork/                       # cover PNGs referenced by reskate-music.json
    cover-0.png, cover-1.png, ...
    playlist-0.png
```

The audio assets (`_mg` / `_nwa` EBX and the Opus audio chunks) live inside `cas_01.cas`, described by
`bam_levelroot.toc`. The `reskate-music.json` and `artwork/` files are **sidecar files**: ReSkate reads
them from the mod folder on disk, like `manifest.json`.

## `reskate-music.json`

Describes your playlists and their covers. `schema` is `1`.

```json
{
  "schema": 1,
  "playlists": [
    {
      "name": "My Mix",
      "songs": ["MGMT - Kids", "Liver Party!! - We Love Your Bloody Clothes"],
      "artwork": "artwork/playlist-0.png"
    }
  ],
  "song_artwork": {
    "MGMT - Kids": "artwork/cover-0.png"
  }
}
```

- `playlists[].name` - the playlist as shown in the in-game music menu.
- `playlists[].songs` - song ids, each exactly `Artist - Title` as it appears in the music metadata.
- `playlists[].artwork` - optional cover, a path **relative to the mod folder**.
- `song_artwork` - optional map of song id to a relative cover path.

The `Artist - Title` id must match the song's tags, or the song will not line up with the playlist.

## Artwork

- Format: a square PNG; the packer produces 512x512.
- Path: relative to the mod folder; `.png`; must not start with `/`, and may not contain `..`,
  backslashes, `:`, `%`, `?` or `#`.
- The runtime caps each image at 4 MiB and 2048x2048, and all mod artwork at 64 MiB total.
- Artwork needs a ReSkate runtime with mod-cover support; older runtimes ignore the artwork fields and
  still load the music.

## Audio

- Stereo 48 kHz Opus, one 20 ms packet per block, in EA's block stream (an `H` header, one `D` block
  per packet, an `E` end), plus a prefetch chunk carrying the seek table.
- Default bitrate 192 kbps (64-320). Normalised to -15 LUFS unless `--no-normalize` is used.

## What ReSkate needs

- The runtime must read `reskate-music.json` to create the mod's playlist. With that alone the songs
  play under your playlist.
- Cover art additionally needs the runtime's mod-artwork support.
- A song id (`Artist - Title`) must be unique; the packer's [clash warning](troubleshooting.md#song-clash-warning)
  helps you avoid colliding with an existing song.
