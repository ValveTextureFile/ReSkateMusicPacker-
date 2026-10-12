// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Cover art: images scaled onto a square, embedded album art, and generated text covers.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};

use super::scan::attached_picture;
use super::text::usable_name;
use super::tools::{self, cache_folder, run, run_capture};
use crate::{Error, Result, assets, bail};

/// Proportionally fit into 512x512 with dark padding.
const ARTWORK_FILTER: &str =
    "scale=512:512:force_original_aspect_ratio=decrease,pad=512:512:(ow-iw)/2:(oh-ih)/2:color=0x242035,setsar=1";
/// Thunderstore icons: 256x256 with transparent padding.
pub const ICON_FILTER: &str =
    "scale=256:256:force_original_aspect_ratio=decrease,pad=256:256:(ow-iw)/2:(oh-ih)/2:color=black@0,setsar=1";

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Renders the first video frame of `input` (or stream `map`) through `filter` into a PNG file.
pub fn ffmpeg_png(input: &Path, map: Option<u64>, filter: &str, output: &Path) -> Result<()> {
    let mut command = tools::command("ffmpeg");
    command.args(["-y", "-v", "error", "-i"]).arg(input);
    if let Some(index) = map {
        command.args(["-map", &format!("0:{index}")]);
    }
    command.args(["-an", "-vf", filter, "-frames:v", "1", "-update", "1"]).arg(output);
    run(command)
}

/// A proportionally resized 512x512 PNG with dark padding; shared by packing and previews.
pub fn image_artwork_png(image: &Path) -> Result<Vec<u8>> {
    let temporary = tools::scratch_file("cover", "png")?;
    let result = ffmpeg_png(image, None, ARTWORK_FILTER, &temporary)
        .and_then(|()| std::fs::read(&temporary).map_err(|error| Error::io("Cannot read the converted cover", error)));
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Extracts a track's first attached picture into the cache, fitted to 512x512. `None` when it
/// has none or it cannot be read: missing or damaged optional artwork never stops a build.
pub fn embedded_artwork(track: &Path) -> Option<PathBuf> {
    let key = format!("{}_cover_v2_fit", tools::sha1_of_file(track).ok()?.hex());
    let cached = cache_folder().join(format!("{key}.png"));
    if let Ok(bytes) = std::fs::read(&cached)
        && bytes.len() > 33
        && bytes.starts_with(PNG_MAGIC)
    {
        return Some(cached);
    }
    let mut probe = tools::command("ffprobe");
    probe
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=index:stream_disposition=attached_pic"])
        .args(["-of", "json"])
        .arg(track);
    let root: serde_json::Value = serde_json::from_str(&run_capture(probe).ok()?).ok()?;
    let index = attached_picture(&root)?;
    let temporary = cache_folder().join(format!("{key}.{}.tmp.png", std::process::id()));
    if ffmpeg_png(track, Some(index), ARTWORK_FILTER, &temporary).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return None;
    }
    let _ = std::fs::remove_file(&cached);
    match std::fs::rename(&temporary, &cached) {
        Ok(()) => Some(cached),
        Err(_) => {
            let _ = std::fs::remove_file(&temporary);
            None
        }
    }
}

// ---- PNG -----------------------------------------------------------------------------------------

pub fn encode_png(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let failure = |error: png::EncodingError| Error::msg(format!("Could not encode a PNG: {error}"));
    let mut writer = encoder.write_header().map_err(failure)?;
    writer.write_image_data(pixels).map_err(failure)?;
    writer.finish().map_err(failure)?;
    Ok(out)
}

/// Decodes a PNG to 8-bit RGBA: (pixels, width, height).
pub fn decode_png_rgba(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let failure = |error: png::DecodingError| Error::msg(format!("Could not read the cover image: {error}"));
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(failure)?;
    let size = reader.output_buffer_size().ok_or_else(|| Error::msg("The cover image is too large"))?;
    let mut buffer = vec![0u8; size];
    let info = reader.next_frame(&mut buffer).map_err(failure)?;
    buffer.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Indexed => bail!("Could not read the cover image: unexpanded palette"),
    };
    Ok((rgba, info.width, info.height))
}

/// The default 256x256 Thunderstore icon: a plain gradient.
pub fn default_icon_png() -> Result<Vec<u8>> {
    let mut pixels = Vec::with_capacity(256 * 256 * 3);
    for y in 0..256u32 {
        for x in 0..256u32 {
            pixels.push((40 + x * 60 / 255) as u8);
            pixels.push((30 + y * 40 / 255) as u8);
            pixels.push((80 + (x + y) * 120 / 510) as u8);
        }
    }
    encode_png(256, 256, png::ColorType::Rgb, &pixels)
}

// ---- Generated text covers -------------------------------------------------------------------------

/// A font and the system fonts that stand in for characters it lacks.
struct Typeface {
    primary: FontArc,
}

fn fallbacks() -> &'static [FontArc] {
    static FALLBACKS: OnceLock<Vec<FontArc>> = OnceLock::new();
    FALLBACKS.get_or_init(|| {
        assets::fallback_fonts()
            .into_iter()
            .filter_map(|(path, index)| {
                let bytes = std::fs::read(path).ok()?;
                let font = ab_glyph::FontVec::try_from_vec_and_index(bytes.clone(), index)
                    .or_else(|_| ab_glyph::FontVec::try_from_vec_and_index(bytes, 0))
                    .ok()?;
                Some(FontArc::new(font))
            })
            .collect()
    })
}

fn em_scale(font: &FontArc, em: f32) -> PxScale {
    let units = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(em * font.height_unscaled() / units)
}

/// One glyph of laid-out text: which font (0 = primary, then the fallbacks), which glyph, where.
struct Placed {
    font: usize,
    id: GlyphId,
    x: f32,
}

impl Typeface {
    fn new(bytes: &'static [u8]) -> Self {
        Typeface { primary: FontArc::try_from_slice(bytes).expect("embedded font is valid") }
    }

    fn font(&self, index: usize) -> &FontArc {
        if index == 0 { &self.primary } else { &fallbacks()[index - 1] }
    }

    fn font_for(&self, c: char) -> (usize, GlyphId) {
        let id = self.primary.glyph_id(c);
        if id.0 != 0 || c.is_whitespace() {
            return (0, id);
        }
        fallbacks()
            .iter()
            .enumerate()
            .find_map(|(index, font)| {
                let id = font.glyph_id(c);
                (id.0 != 0).then_some((index + 1, id))
            })
            .unwrap_or((0, id))
    }

    /// Glyphs with their pen positions from 0, and the total advance.
    fn layout(&self, text: &str, em: f32) -> (Vec<Placed>, f32) {
        let mut placed = Vec::new();
        let mut x = 0.0;
        let mut previous: Option<(usize, GlyphId)> = None;
        for c in text.chars() {
            let (font, id) = self.font_for(c);
            let scaled = self.font(font).as_scaled(em_scale(self.font(font), em));
            if let Some((last_font, last_id)) = previous
                && last_font == font
            {
                x += scaled.kern(last_id, id);
            }
            placed.push(Placed { font, id, x });
            x += scaled.h_advance(id);
            previous = Some((font, id));
        }
        (placed, x)
    }

    fn width(&self, text: &str, em: f32) -> f32 {
        self.layout(text, em).1
    }

    /// (ascent, line height) in pixels.
    fn metrics(&self, em: f32) -> (f32, f32) {
        let scaled = self.primary.as_scaled(em_scale(&self.primary, em));
        (scaled.ascent(), (scaled.ascent() - scaled.descent()).round())
    }
}

/// Splits text into lines no wider than `width`, breaking after the last space that fits, or
/// mid-word when a word alone is too wide.
fn wrap(face: &Typeface, text: &str, width: f32, em: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while !rest.is_empty() {
        let (mut count, mut space) = (0, 0);
        for end in 1..=rest.len() {
            let prefix: String = rest[..end].iter().collect();
            if face.width(&prefix, em) > width {
                break;
            }
            count = end;
            if rest[end - 1] == ' ' {
                space = end;
            }
        }
        // Break after the last space that fits, unless the line already ends at a word boundary.
        if count < rest.len() && rest[count] != ' ' && space > 0 {
            count = space;
        }
        count = count.max(1);
        let line: String = rest[..count].iter().collect();
        lines.push(line.trim_end_matches(' ').to_owned());
        rest.drain(..count);
        while rest.first() == Some(&' ') {
            rest.remove(0);
        }
    }
    lines
}

const SIZE: usize = 512;

struct Canvas {
    rgb: Vec<[u8; 3]>,
}

struct Clip {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Canvas {
    fn draw_text(&mut self, face: &Typeface, text: &str, x: f32, baseline: f32, em: f32, clip: &Clip) {
        let (glyphs, _) = face.layout(text, em);
        for glyph in glyphs {
            let font = face.font(glyph.font);
            let positioned = glyph.id.with_scale_and_position(em_scale(font, em), point(x + glyph.x, baseline));
            let Some(outlined) = font.outline_glyph(positioned) else { continue };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < clip.left || px >= clip.right || py < clip.top || py >= clip.bottom {
                    return;
                }
                let pixel = &mut self.rgb[py as usize * SIZE + px as usize];
                let alpha = coverage.clamp(0.0, 1.0);
                for channel in pixel.iter_mut() {
                    *channel = (*channel as f32 + (255.0 - *channel as f32) * alpha).round() as u8;
                }
            });
        }
    }
}

/// A deterministic 512x512 text cover: "PLAYLIST" and the name over a gradient. Also used by the
/// window's preview.
pub fn playlist_artwork_png(name: &str) -> Result<Vec<u8>> {
    static BOLD: OnceLock<Typeface> = OnceLock::new();
    static SEMIBOLD: OnceLock<Typeface> = OnceLock::new();
    if !usable_name(name) {
        bail!("A generated cover needs a valid playlist name");
    }
    let bold = BOLD.get_or_init(|| Typeface::new(assets::INTER_BOLD));
    let semibold = SEMIBOLD.get_or_init(|| Typeface::new(assets::INTER_SEMIBOLD));

    let mut canvas = Canvas { rgb: Vec::with_capacity(SIZE * SIZE) };
    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            let accent = (x - 450) * (x - 450) + (y - 40) * (y - 40) < 170 * 170;
            let lift = |amount: i32| if accent { amount } else { 0 };
            canvas.rgb.push([
                (45 + y * 15 / 512 + lift(25)) as u8,
                (35 + x * 50 / 512 + lift(15)) as u8,
                (80 + y * 50 / 512 + lift(20)) as u8,
            ]);
        }
    }

    let (ascent, _) = semibold.metrics(22.0);
    let badge = Clip { left: 48, top: 48, right: 464, bottom: 84 };
    canvas.draw_text(semibold, "PLAYLIST", 48.0, 48.0 + ascent, 22.0, &badge);

    // The largest size, in steps of 2 from 64 down to 16, at which the name fits 416x280.
    for size in (16..=64).rev().step_by(2) {
        let em = size as f32;
        let lines = wrap(bold, name, 416.0, em);
        let (ascent, line_height) = bold.metrics(em);
        let height = lines.len() as i32 * line_height as i32;
        if height > 280 && size > 16 {
            continue;
        }
        let top = 150 + (280 - height) / 2;
        let clip = Clip { left: 48, top, right: 464, bottom: 430 };
        for (index, line) in lines.iter().enumerate() {
            let x = 48.0 + ((416.0 - bold.width(line, em)) / 2.0).floor();
            let baseline = top as f32 + index as f32 * line_height + ascent;
            canvas.draw_text(bold, line, x, baseline, em, &clip);
        }
        break;
    }

    let pixels: Vec<u8> = canvas.rgb.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
    encode_png(SIZE as u32, SIZE as u32, png::ColorType::Rgba, &pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_covers_are_512_square_pngs() {
        let png = playlist_artwork_png("A Rather Long Playlist Name That Has To Wrap Over Lines").unwrap();
        let (pixels, width, height) = decode_png_rgba(&png).unwrap();
        assert_eq!((width, height), (512, 512));
        // The text is white: some pixels inside the title area are much brighter than the gradient.
        assert!(pixels.chunks_exact(4).any(|p| p[0] > 230 && p[1] > 230 && p[2] > 230));
        assert!(playlist_artwork_png("").is_err());
        assert_eq!(png, playlist_artwork_png("A Rather Long Playlist Name That Has To Wrap Over Lines").unwrap());
    }

    #[test]
    fn wrapping_prefers_spaces() {
        let face = Typeface::new(assets::INTER_BOLD);
        let lines = wrap(&face, "aaaa bbbb cccc", face.width("aaaa bbbb", 20.0) + 1.0, 20.0);
        assert_eq!(lines, ["aaaa bbbb", "cccc"]);
        let long = wrap(&face, "x".repeat(50).as_str(), 100.0, 20.0);
        assert!(long.len() > 1 && long.iter().all(|line| face.width(line, 20.0) <= 100.0));
    }

    #[test]
    fn default_icon_decodes() {
        let (_, width, height) = decode_png_rgba(&default_icon_png().unwrap()).unwrap();
        assert_eq!((width, height), (256, 256));
    }
}
