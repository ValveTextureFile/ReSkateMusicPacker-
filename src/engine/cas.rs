// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-FileCopyrightText: 2026 the ReSkate contributors
// SPDX-License-Identifier: GPL-3.0-only
// Ported from the ReSkate project (https://github.com/Dingo-Shenanigans/ReSkate); see NOTICE.md.
//! The CAS block stream: each block is an 8-byte header (decoded size, codec, encoded size)
//! followed by its payload.

use std::path::{Path, PathBuf};

use super::oodle;
use crate::{Error, Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CasCompression {
    Raw = 0x00,
    Kraken = 0x11,
    Selkie = 0x15,
    Leviathan = 0x19,
}

impl CasCompression {
    fn oodle_compressor(self) -> Option<i32> {
        match self {
            CasCompression::Raw => None,
            CasCompression::Kraken => Some(8),
            CasCompression::Selkie => Some(11),
            CasCompression::Leviathan => Some(13),
        }
    }
}

pub struct DecodeOptions<'a> {
    pub game_root: &'a Path,
    pub maximum_output: usize,
}

impl<'a> DecodeOptions<'a> {
    pub fn new(game_root: &'a Path) -> Self {
        DecodeOptions { game_root, maximum_output: 256 * 1024 * 1024 }
    }
}

#[derive(Clone, Debug)]
pub struct EncodeOptions {
    pub game_root: PathBuf,
    pub compression: CasCompression,
    pub level: i32,
    pub block_size: usize,
}

impl EncodeOptions {
    pub fn new(game_root: impl Into<PathBuf>, compression: CasCompression) -> Self {
        EncodeOptions { game_root: game_root.into(), compression, level: 4, block_size: 0x10000 }
    }
}

fn take<'a>(input: &mut &'a [u8], size: usize) -> Result<&'a [u8]> {
    if size > input.len() {
        bail!("Truncated CAS stream");
    }
    let (head, tail) = input.split_at(size);
    *input = tail;
    Ok(head)
}

fn read_block(input: &mut &[u8], options: &DecodeOptions<'_>) -> Result<Vec<u8>> {
    if input.len() < 8 {
        bail!("Truncated CAS block header");
    }
    let header = take(input, 8)?;
    let mut decoded_size = u32::from_be_bytes(header[0..4].try_into().unwrap());
    let compression = header[4] & 0x7F;
    let flags = header[5];
    let encoded_size = u16::from_be_bytes([header[6], header[7]]) as usize + (((flags & 0x0F) as usize) << 16);
    let uses_dictionary = decoded_size & 0xFF00_0000 != 0;
    decoded_size &= 0x00FF_FFFF;
    let size = decoded_size as usize;
    if size > options.maximum_output {
        bail!("CAS block is too large");
    }
    let payload = take(input, encoded_size)?;
    match compression {
        0x00 => {
            if payload.len() != size {
                bail!("Raw CAS block size does not match its header");
            }
            Ok(payload.to_vec())
        }
        0x02 => {
            let output = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(payload, size)
                .map_err(|_| Error::msg("zlib CAS block failed to decode"))?;
            if output.len() != size {
                bail!("zlib CAS block failed to decode");
            }
            Ok(output)
        }
        0x09 => {
            let mut output = vec![0u8; size];
            match lz4_flex::block::decompress_into(payload, &mut output) {
                Ok(written) if written == size => Ok(output),
                _ => bail!("LZ4 CAS block failed to decode"),
            }
        }
        0x0F => {
            if uses_dictionary {
                bail!("Dictionary Zstandard CAS blocks are not supported");
            }
            let mut output = vec![0u8; size];
            match ruzstd::decoding::FrameDecoder::new().decode_all(payload, &mut output) {
                Ok(written) if written == size => Ok(output),
                Ok(_) => bail!("Zstandard CAS block failed to decode: short output"),
                Err(error) => bail!("Zstandard CAS block failed to decode: {error}"),
            }
        }
        0x11 | 0x15 | 0x19 => oodle::decompress(payload, size, options.game_root),
        other => bail!("Unsupported CAS compression type {other}"),
    }
}

pub fn decode_cas(mut encoded: &[u8], options: &DecodeOptions<'_>) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    while !encoded.is_empty() {
        let block = read_block(&mut encoded, options)?;
        if block.len() > options.maximum_output - output.len() {
            bail!("CAS output exceeds the safety limit");
        }
        output.extend_from_slice(&block);
    }
    Ok(output)
}

pub fn encode_cas(decoded: &[u8], options: &EncodeOptions) -> Result<Vec<u8>> {
    if options.block_size == 0 || options.block_size > 0x10000 {
        bail!("CAS block size must be between 1 and 65536 bytes");
    }
    let mut output = Vec::with_capacity(decoded.len() + decoded.len() / options.block_size * 8 + 8);
    // An empty input is still one (empty) block.
    let blocks: Vec<&[u8]> =
        if decoded.is_empty() { vec![decoded] } else { decoded.chunks(options.block_size).collect() };
    for input in blocks {
        let mut compression = options.compression;
        let mut payload = match compression.oodle_compressor() {
            Some(compressor) if !input.is_empty() => {
                oodle::compress(input, compressor, options.level, &options.game_root)?
            }
            _ => None,
        };
        // A block that did not shrink, or could not be compressed here, is stored raw, exactly as
        // the game does.
        if payload.as_ref().is_none_or(|bytes| bytes.len() >= input.len()) {
            payload = Some(input.to_vec());
            compression = CasCompression::Raw;
        }
        let payload = payload.expect("set above");
        if payload.len() > 0xFFFFF {
            bail!("Encoded CAS block is too large");
        }
        let packed = ((input.len() as u64 & 0xFF_FFFF) << 32)
            | ((compression as u64) << 24)
            | (0x7 << 20)
            | payload.len() as u64;
        output.extend_from_slice(&packed.to_be_bytes());
        output.extend_from_slice(&payload);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_blocks_round_trip() {
        let data: Vec<u8> = (0..200_000u32).map(|value| (value * 7 % 251) as u8).collect();
        let encoded = encode_cas(&data, &EncodeOptions::new("", CasCompression::Raw)).unwrap();
        assert_eq!(encoded.len(), data.len() + 4 * 8);
        assert_eq!(&encoded[..8], &[0x00, 0x01, 0x00, 0x00, 0x00, 0x71, 0x00, 0x00]);
        assert_eq!(decode_cas(&encoded, &DecodeOptions::new(Path::new(""))).unwrap(), data);
    }

    #[test]
    fn an_empty_payload_is_one_empty_block() {
        let encoded = encode_cas(&[], &EncodeOptions::new("", CasCompression::Raw)).unwrap();
        assert_eq!(encoded, [0, 0, 0, 0, 0, 0x70, 0, 0]);
        assert!(decode_cas(&encoded, &DecodeOptions::new(Path::new(""))).unwrap().is_empty());
    }

    #[test]
    fn zlib_and_lz4_blocks_decode() {
        let data = b"hello hello hello hello hello hello".to_vec();
        let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
        let lz4 = lz4_flex::block::compress(&data);
        for (codec, payload) in [(0x02u8, zlib), (0x09, lz4)] {
            let mut block = (data.len() as u32).to_be_bytes().to_vec();
            block.push(codec);
            block.push(0x70);
            block.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            block.extend_from_slice(&payload);
            assert_eq!(decode_cas(&block, &DecodeOptions::new(Path::new(""))).unwrap(), data);
        }
    }
}
