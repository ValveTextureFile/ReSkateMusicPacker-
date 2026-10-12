// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! A song's audio as the game streams it: 48 kHz stereo Opus, one 20 ms packet per block, in EA's
//! block stream (an `H` header, one `D` block per packet, an `E` end) plus a prefetch chunk that
//! carries the stream's seek table.

use std::path::Path;

use super::tools::{self, cache_folder, run, run_capture};
use crate::{Error, Result, bail};

/// Every shipped song's audio packet is one 20 ms Opus frame.
pub const PACKET_SAMPLES: u32 = 960;

#[derive(Clone, Debug, Default)]
pub struct Opus {
    pub pre_skip: u16,
    /// Playable samples: the last granule minus the pre-skip.
    pub samples: u64,
    pub packets: Vec<Vec<u8>>,
}

/// Samples in one Opus packet, from its TOC byte (RFC 6716 section 3.1).
pub fn samples_in(packet: &[u8]) -> Result<u32> {
    let Some(&toc) = packet.first() else { bail!("Empty Opus packet") };
    let config = (toc >> 3) as usize;
    let frame = if config < 12 {
        [480, 960, 1920, 2880][config & 3]
    } else if config < 16 {
        if config & 1 != 0 { 960 } else { 480 }
    } else {
        [120, 240, 480, 960][config & 3]
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => match packet.get(1) {
            Some(count) => (count & 0x3F) as u32,
            None => bail!("Truncated Opus packet"),
        },
    };
    Ok(frame * frames)
}

/// The packets of an Ogg Opus file: OpusHead first, then OpusTags, then audio.
pub fn read_ogg_opus(ogg: &[u8]) -> Result<Opus> {
    let mut packets: Vec<Vec<u8>> = Vec::new();
    let mut partial = Vec::new();
    let mut granule: i64 = 0;
    let mut at = 0;
    while at + 27 <= ogg.len() {
        if &ogg[at..at + 4] != b"OggS" {
            bail!("Not an Ogg stream");
        }
        let page_granule = i64::from_le_bytes(ogg[at + 6..at + 14].try_into().expect("8 bytes"));
        let segments = ogg[at + 26] as usize;
        if at + 27 + segments > ogg.len() {
            bail!("Truncated Ogg page");
        }
        let table = &ogg[at + 27..at + 27 + segments];
        let mut data = at + 27 + segments;
        for &length in table {
            let length = length as usize;
            if data + length > ogg.len() {
                bail!("Truncated Ogg page");
            }
            partial.extend_from_slice(&ogg[data..data + length]);
            data += length;
            if length < 255 {
                packets.push(std::mem::take(&mut partial));
            }
        }
        if page_granule >= 0 {
            granule = page_granule;
        }
        at = data;
    }
    if packets.len() < 3 || packets[0].len() < 19 || &packets[0][..8] != b"OpusHead" {
        bail!("Not an Ogg Opus stream");
    }
    if packets[0][9] != 2 {
        bail!("Expected stereo Opus");
    }
    let pre_skip = u16::from_le_bytes([packets[0][10], packets[0][11]]);
    if granule <= pre_skip as i64 {
        bail!("Opus stream has no audio");
    }
    Ok(Opus { pre_skip, samples: granule as u64 - pre_skip as u64, packets: packets.split_off(2) })
}

/// What each D block yields: the first leaves out the encoder's pre-skip, the last any padding
/// past the stream's end.
fn block_yields(opus: &Opus) -> impl Iterator<Item = (usize, u64)> + '_ {
    let mut remaining = opus.samples;
    (0..opus.packets.len()).map_while(move |index| {
        if remaining == 0 {
            return None;
        }
        let skip = if index == 0 { (opus.pre_skip as u32).min(PACKET_SAMPLES) } else { 0 };
        let yields = ((PACKET_SAMPLES - skip) as u64).min(remaining);
        remaining -= yields;
        Some((index, yields))
    })
}

/// The playable sample total: exactly what the D blocks carry. This is what the shipped songs
/// store in the codec header, so it must come from the packets, not from the Ogg granule (ffmpeg
/// writes the input duration there, which can exceed the decoded packet sum).
pub fn playable_samples(opus: &Opus) -> u64 {
    block_yields(opus).map(|(_, yields)| yields).sum()
}

/// The 10-byte codec header: the template's, with the playable sample count in bits 6..31 of
/// bytes 4..8.
pub fn codec_header(template: &[u8; 10], samples: u64) -> Result<[u8; 10]> {
    if samples >= 1 << 26 {
        bail!("Song too long");
    }
    let mut header = *template;
    let word = u32::from_be_bytes(header[4..8].try_into().expect("4 bytes"));
    let word = ((samples as u32) << 6) | (word & 0x3F);
    header[4..8].copy_from_slice(&word.to_be_bytes());
    Ok(header)
}

/// The sample count stored in a codec header.
pub fn header_samples(header: &[u8; 10]) -> u64 {
    (u32::from_be_bytes(header[4..8].try_into().expect("4 bytes")) >> 6) as u64
}

fn put_block(out: &mut Vec<u8>, id: u8, payload: &[u8]) -> Result<()> {
    let size = payload.len() + 4;
    if size > 0xFF_FFFF {
        bail!("Audio block too large");
    }
    out.extend_from_slice(&(((id as u32) << 24) | size as u32).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// H header, one D block per packet (u32 BE samples it yields + the packet), E end.
pub fn stream_chunk(opus: &Opus, header: &[u8; 10]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put_block(&mut out, b'H', header)?;
    for (index, yields) in block_yields(opus) {
        let mut payload = (yields as u32).to_be_bytes().to_vec();
        payload.extend_from_slice(&opus.packets[index]);
        put_block(&mut out, b'D', &payload)?;
    }
    put_block(&mut out, b'E', &[0])?;
    Ok(out)
}

// ---- Seek table ----------------------------------------------------------------------------------
// The prefetch chunk's body is the stream's seek table, which the game walks to start a song part
// way through (the prefetch parser at 0x14137db30; version 1, table type 1 = byte 1's high nibble,
// handed to 0x14137dec0). One entry per H and D block: {block size in bytes, 0, samples the block
// yields, 1 on the first D block only}. Each column is its own stream of runs ("n >= 0": n + 1
// entries of one value, one delta) and literal lists ("n < 0": 1 - n entries, one delta each),
// read in entry order: for each entry, column 0, 1, 2, 3, each reading its next header when the
// last ran out. Checked against 92,553 entries of 12 shipped songs.

pub type SeekEntry = [i64; 4];

/// Signed varint (0x14137f720): the sign is the last byte's low bit and stores !v. |v| < 96 is one
/// byte (|v| * 2 + sign); |v| < 6240 two (0xC000 + ((|v| - 96) << 1 | sign), big endian). The game
/// also reads 3- to 5-byte forms; the table below never needs them.
fn put_varint(out: &mut Vec<u8>, value: i64) -> Result<()> {
    let sign = (value < 0) as i64;
    let magnitude = if value < 0 { !value } else { value };
    if magnitude < 96 {
        out.push((magnitude * 2 + sign) as u8);
        return Ok(());
    }
    if magnitude - 96 >= 0x1800 {
        bail!("Seek table value out of range");
    }
    let encoded = (0xC000 + (((magnitude - 96) << 1) | sign)) as u16;
    out.extend_from_slice(&encoded.to_be_bytes());
    Ok(())
}

fn get_varint(input: &[u8], at: &mut usize) -> Result<i64> {
    let Some(&first) = input.get(*at) else { bail!("Seek table ends early") };
    if first < 0xC0 {
        *at += 1;
        let magnitude = (first >> 1) as i64;
        return Ok(if first & 1 != 0 { !magnitude } else { magnitude });
    }
    if first >= 0xF0 || *at + 1 >= input.len() {
        bail!("Seek table varint form not written by this tool");
    }
    let encoded = u16::from_be_bytes([first, input[*at + 1]]) as i64;
    *at += 2;
    let magnitude = ((encoded >> 1) & 0x1FFF) + 96;
    Ok(if encoded & 1 != 0 { !magnitude } else { magnitude })
}

pub fn seek_entries(stream: &[u8]) -> Result<Vec<SeekEntry>> {
    let mut entries: Vec<SeekEntry> = Vec::new();
    let mut at = 0;
    while at + 4 <= stream.len() {
        let id = stream[at];
        let size = u32::from_be_bytes([0, stream[at + 1], stream[at + 2], stream[at + 3]]) as usize;
        if id == b'E' {
            break;
        }
        if (id != b'H' && id != b'D') || size < 4 || at + size > stream.len() {
            bail!("Unexpected stream block");
        }
        let samples = if id == b'D' {
            if size < 8 {
                bail!("Unexpected stream block");
            }
            u32::from_be_bytes(stream[at + 4..at + 8].try_into().expect("4 bytes")) as i64
        } else {
            0
        };
        let first_data = (entries.len() == 1) as i64;
        entries.push([size as i64, 0, samples, first_data]);
        at += size;
    }
    Ok(entries)
}

pub fn seek_table(entries: &[SeekEntry]) -> Result<Vec<u8>> {
    // Each column as runs of equal values (all but the sizes, which vary per block) and literal
    // lists, at most 6000 entries each so every header fits the two-byte varint.
    struct Segment {
        run: bool,
        values: Vec<i64>,
    }
    const MOST: usize = 6000;
    let mut segments: [Vec<Segment>; 4] = Default::default();
    for (column, out) in segments.iter_mut().enumerate() {
        let mut i = 0;
        while i < entries.len() {
            let mut j = i;
            while j < entries.len() && entries[j][column] == entries[i][column] && j - i < MOST {
                j += 1;
            }
            if column != 0 && j - i >= 2 {
                out.push(Segment { run: true, values: vec![entries[i][column]; j - i] });
                i = j;
            } else {
                if out.last().is_none_or(|last| last.run || last.values.len() >= MOST) {
                    out.push(Segment { run: false, values: Vec::new() });
                }
                out.last_mut().expect("pushed above").values.push(entries[i][column]);
                i += 1;
            }
        }
        // A one-entry list would need header 0, which means a run: write it as a run of one.
        for segment in out.iter_mut() {
            if !segment.run && segment.values.len() == 1 {
                segment.run = true;
            }
        }
    }

    #[derive(Default, Clone, Copy)]
    struct Cursor {
        segment: usize,
        left: usize,
        next: usize,
        value: i64,
    }
    let mut out = Vec::new();
    let mut cursors = [Cursor::default(); 4];
    for _ in entries {
        for (column, cursor) in cursors.iter_mut().enumerate() {
            if cursor.left == 0 {
                let segment = &segments[column][cursor.segment];
                cursor.segment += 1;
                cursor.left = segment.values.len();
                cursor.next = 0;
                if segment.run {
                    put_varint(&mut out, segment.values.len() as i64 - 1)?;
                    put_varint(&mut out, segment.values[0] - cursor.value)?;
                    cursor.value = segment.values[0];
                } else {
                    put_varint(&mut out, 1 - segment.values.len() as i64)?;
                }
            }
            let segment = &segments[column][cursor.segment - 1];
            if !segment.run {
                put_varint(&mut out, segment.values[cursor.next] - cursor.value)?;
                cursor.value = segment.values[cursor.next];
            }
            cursor.next += 1;
            cursor.left -= 1;
        }
    }
    Ok(out)
}

/// Walks a table as 0x14137dec0 does and checks it gives back every entry.
pub fn check_seek_table(table: &[u8], entries: &[SeekEntry]) -> Result<()> {
    #[derive(Default, Clone, Copy)]
    struct Column {
        value: i64,
        left: i64,
        run: bool,
    }
    let mut columns = [Column::default(); 4];
    let mut at = 0;
    for entry in entries {
        for (column, state) in columns.iter_mut().enumerate() {
            if state.left <= 0 {
                let n = get_varint(table, &mut at)?;
                state.run = n >= 0;
                state.left = if state.run { n + 1 } else { 1 - n };
                if state.run {
                    state.value += get_varint(table, &mut at)?;
                }
            }
            if !state.run {
                state.value += get_varint(table, &mut at)?;
            }
            state.left -= 1;
            if state.value != entry[column] {
                bail!("Seek table does not read back");
            }
        }
    }
    Ok(())
}

/// The prefetch chunk: [01 10][u16 BE pre-skip][u32 BE 22, the table's offset][u32 0, no optional
/// data][codec header][seek table], zero-padded to 4 bytes like the shipped chunks.
pub fn prefetch_chunk(template: &[u8], opus: &Opus, header: &[u8; 10], stream: &[u8]) -> Result<Vec<u8>> {
    if template.len() < 22 || template[0] != 1 || template[1] != 0x10 || template[7] != 22 {
        bail!("Template prefetch chunk has an unexpected layout");
    }
    let mut out = template[..12].to_vec();
    out[2..4].copy_from_slice(&opus.pre_skip.to_be_bytes());
    out.extend_from_slice(header);
    let entries = seek_entries(stream)?;
    let table = seek_table(&entries)?;
    check_seek_table(&table, &entries)?;
    out.extend_from_slice(&table);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    Ok(out)
}

// ---- Encoding ------------------------------------------------------------------------------------

/// The game's own tracks sit at about -15 LUFS (measured from the shipped UPM and licensed
/// streams), so normalise to that. Two passes: measure the input, then apply a linear gain so each
/// song lands on the target instead of the single-pass dynamic loudnorm's drift. The cache tag
/// changes with the target so an older -16 encode is not reused.
const LOUDNESS_FILTER: &str = "I=-15.0:TP=-1.5:LRA=11";
const LOUDNESS_CACHE_TAG: &str = "_norm15";

fn loudness_filter(file: &Path) -> String {
    let base = format!("loudnorm={LOUDNESS_FILTER}");
    let mut command = tools::command("ffmpeg");
    command
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-vn", "-map_metadata", "-1", "-ac", "2", "-ar", "48000", "-af"])
        .arg(format!("{base}:print_format=json"))
        .args(["-f", "null", "-"]);
    // Measurement failing falls back to the dynamic filter.
    let Ok(text) = run_capture(command) else { return base };
    let (Some(start), Some(end)) = (text.rfind('{'), text.rfind('}')) else { return base };
    if end < start {
        return base;
    }
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&text[start..=end]) else { return base };
    let value = |key: &str| root.get(key).and_then(|value| value.as_str()).unwrap_or_default().to_owned();
    let (i, lra, tp) = (value("input_i"), value("input_lra"), value("input_tp"));
    let (thresh, offset) = (value("input_thresh"), value("target_offset"));
    if i.is_empty() || tp.is_empty() {
        return base;
    }
    format!(
        "{base}:measured_I={i}:measured_LRA={lra}:measured_TP={tp}:measured_thresh={thresh}:offset={offset}:linear=true"
    )
}

fn all_packets_are_20ms(opus: &Opus) -> bool {
    opus.packets.iter().all(|packet| samples_in(packet).is_ok_and(|samples| samples == PACKET_SAMPLES))
}

pub struct Encoded {
    pub opus: Opus,
    pub cached: bool,
}

/// Encodes a song to 20 ms Opus, reusing the cached encode of the same file and settings.
pub fn encode(file: &Path, bitrate: u32, normalize: bool) -> Result<Encoded> {
    let file_sha = tools::sha1_of_file(file)?.hex();
    let tag = if normalize { LOUDNESS_CACHE_TAG } else { "" };
    let cached_file = cache_folder().join(format!("{file_sha}_{bitrate}k{tag}.ogg"));

    if cached_file.is_file() {
        let cached = std::fs::read(&cached_file).ok().and_then(|bytes| read_ogg_opus(&bytes).ok());
        if let Some(opus) = cached.filter(|opus| !opus.packets.is_empty() && all_packets_are_20ms(opus)) {
            return Ok(Encoded { opus, cached: true });
        }
        // A corrupt cache entry is re-encoded.
        let _ = std::fs::remove_file(&cached_file);
    }

    let ogg = tools::scratch_file(&format!("{file_sha}_{bitrate}k{tag}"), "ogg")?;
    let mut command = tools::command("ffmpeg");
    command.args(["-y", "-v", "error", "-i"]).arg(file).args([
        "-vn",
        "-map_metadata",
        "-1",
        "-ac",
        "2",
        "-ar",
        "48000",
    ]);
    if normalize {
        command.arg("-af").arg(loudness_filter(file));
    }
    command
        .args(["-c:a", "libopus", "-b:a", &format!("{bitrate}k"), "-vbr", "on", "-frame_duration", "20"])
        .args(["-application", "audio", "-f", "ogg"])
        .arg(&ogg);
    let result = run(command).and_then(|()| {
        let bytes = std::fs::read(&ogg).map_err(|error| Error::io("Cannot read ffmpeg's output", error))?;
        let opus = read_ogg_opus(&bytes)?;
        if !all_packets_are_20ms(&opus) {
            bail!("ffmpeg produced a packet that is not 20 ms");
        }
        Ok(opus)
    });
    let opus = match result {
        Ok(opus) => opus,
        Err(error) => {
            let _ = std::fs::remove_file(&ogg);
            return Err(error);
        }
    };
    if std::fs::rename(&ogg, &cached_file).is_err() {
        let _ = std::fs::copy(&ogg, &cached_file);
        let _ = std::fs::remove_file(&ogg);
    }
    Ok(Encoded { opus, cached: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opus(packets: usize, pre_skip: u16, samples: u64) -> Opus {
        // TOC 0xFC: config 31 (CELT FB 20 ms), one frame.
        Opus { pre_skip, samples, packets: (0..packets).map(|index| vec![0xFC, index as u8, 1, 2]).collect() }
    }

    #[test]
    fn packet_durations_come_from_the_toc_byte() {
        assert_eq!(samples_in(&[0xFC]).unwrap(), 960);
        assert_eq!(samples_in(&[0x0B << 3]).unwrap(), 2880); // SILK 60 ms
        assert_eq!(samples_in(&[(31 << 3) | 3, 2]).unwrap(), 1920);
        assert!(samples_in(&[]).is_err());
    }

    #[test]
    fn blocks_leave_out_pre_skip_and_padding() {
        let song = opus(10, 312, 960 * 8);
        assert_eq!(playable_samples(&song), 960 * 8);
        let header = codec_header(&[1, 2, 3, 4, 0, 0, 0, 0x2A, 9, 10], playable_samples(&song)).unwrap();
        assert_eq!(header_samples(&header), 960 * 8);
        assert_eq!(header[7] & 0x3F, 0x2A);
        let stream = stream_chunk(&song, &header).unwrap();
        let entries = seek_entries(&stream).unwrap();
        // H, then D blocks until the samples run out.
        assert_eq!(entries[0], [14, 0, 0, 0]);
        assert_eq!(entries[1][2], 960 - 312);
        assert_eq!(entries[1][3], 1);
        assert_eq!(entries.iter().map(|entry| entry[2]).sum::<i64>(), 960 * 8);
        assert_eq!(&stream[stream.len() - 5..], &[b'E', 0, 0, 5, 0]);
    }

    #[test]
    fn seek_tables_read_back() {
        let song = opus(20_000, 312, 960 * 19_990);
        let header = codec_header(&[0; 10], playable_samples(&song)).unwrap();
        let stream = stream_chunk(&song, &header).unwrap();
        let entries = seek_entries(&stream).unwrap();
        let table = seek_table(&entries).unwrap();
        check_seek_table(&table, &entries).unwrap();
        let template = [1, 0x10, 0, 0, 0, 0, 0, 22, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let prefetch = prefetch_chunk(&template, &song, &header, &stream).unwrap();
        assert_eq!(&prefetch[2..4], &312u16.to_be_bytes());
        assert_eq!(prefetch.len() % 4, 0);
    }

    #[test]
    fn varints_round_trip() {
        for value in [-6239, -97, -96, -1, 0, 1, 95, 96, 6239] {
            let mut out = Vec::new();
            put_varint(&mut out, value).unwrap();
            let mut at = 0;
            assert_eq!(get_varint(&out, &mut at).unwrap(), value);
            assert_eq!(at, out.len());
        }
        assert!(put_varint(&mut Vec::new(), 7000).is_err());
    }

    #[test]
    fn ogg_pages_split_into_packets() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xBB, 0, 0, 0, 0, 0]);
        let packets = [head, b"OpusTags".to_vec(), vec![0xFC; 300], vec![0xFC; 10]];
        let mut page = b"OggS".to_vec();
        page.extend_from_slice(&[0, 0]);
        page.extend_from_slice(&(2000i64).to_le_bytes());
        page.extend_from_slice(&[0; 12]);
        let lacing: Vec<u8> = vec![19, 8, 255, 45, 10];
        page.push(lacing.len() as u8);
        page.extend_from_slice(&lacing);
        for packet in &packets {
            page.extend_from_slice(packet);
        }
        let opus = read_ogg_opus(&page).unwrap();
        assert_eq!(opus.pre_skip, 312);
        assert_eq!(opus.samples, 2000 - 312);
        assert_eq!(opus.packets.len(), 2);
        assert_eq!(opus.packets[0].len(), 300);
    }
}
