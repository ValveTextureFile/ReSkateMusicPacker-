// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-FileCopyrightText: 2026 the ReSkate contributors
// SPDX-License-Identifier: GPL-3.0-only
// Ported from the ReSkate project (https://github.com/Dingo-Shenanigans/ReSkate); see NOTICE.md.
//! Superbundle TOC reader and patch writer. The writer rebuilds the bundle and chunk perfect
//! hashes, which is what makes a TOC resolvable by the engine.

use std::collections::HashMap;

use super::binary::{BE, Guid, Reader, Writer, align_up, c_bytes_at};
use super::bundle::CasIdentifier;
use super::native_db::{ENVELOPE_SIZE, MAGIC};
use crate::{Error, Result, bail};

#[derive(Clone, Debug)]
pub struct TocBundle {
    pub name: String,
    pub region: Vec<u8>,
    pub load_flag: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TocChunk {
    pub guid: Guid,
    pub location: CasIdentifier,
    pub offset: u32,
    pub size: u32,
    pub removed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TocDocument {
    pub flags: u32,
    pub bundles: Vec<TocBundle>,
    pub chunks: Vec<TocChunk>,
    pub bundle_hash_map: Vec<i32>,
    pub chunk_hash_map: Vec<i32>,
}

const FNV_PRIME: u32 = 0x0100_0193;
const COMPRESSED_NAMES: u32 = 4;

/// FNV-1 over the key's bytes taken as signed chars, reduced modulo the prime.
pub fn toc_hash(key: &[u8], seed: u32) -> u32 {
    let mut value = seed;
    for &byte in key {
        value = value.wrapping_mul(FNV_PRIME) ^ (byte as i8 as i32 as u32);
    }
    value % FNV_PRIME
}

const DEFAULT_SEED: u32 = 0x811C_9DC5;

struct PerfectHash {
    map: Vec<i32>,
    /// Slot -> index of the key that lives there.
    order: Vec<usize>,
}

fn perfect_hash<K: AsRef<[u8]> + Ord>(keys: &[K]) -> Result<PerfectHash> {
    let size = keys.len();
    let mut result = PerfectHash { map: vec![-1; size], order: vec![usize::MAX; size] };
    if size == 0 {
        return Ok(result);
    }
    let mut sorted: Vec<&K> = keys.iter().collect();
    sorted.sort();
    if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("TOC perfect-hash keys must be unique");
    }
    let slot = |key: &K, seed: u32| toc_hash(key.as_ref(), seed) as usize % size;

    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); size];
    for (index, key) in keys.iter().enumerate() {
        buckets[slot(key, DEFAULT_SEED)].push(index);
    }
    // Stable, so equal-sized buckets keep their order, as std::stable_sort does.
    buckets.sort_by_key(|bucket| std::cmp::Reverse(bucket.len()));

    let mut used = vec![false; size];
    let mut bucket_index = 0;
    while bucket_index < buckets.len() && buckets[bucket_index].len() > 1 {
        let bucket = &buckets[bucket_index];
        let mut seed: u32 = 1;
        let mut indices = Vec::with_capacity(bucket.len());
        loop {
            indices.clear();
            let collision = bucket.iter().any(|&member| {
                let index = slot(&keys[member], seed);
                let taken = used[index] || indices.contains(&index);
                indices.push(index);
                taken
            });
            if !collision {
                break;
            }
            seed = seed.checked_add(1).ok_or_else(|| Error::msg("could not build TOC perfect hash"))?;
        }
        result.map[slot(&keys[bucket[0]], DEFAULT_SEED)] = seed as i32;
        for (&member, &index) in bucket.iter().zip(&indices) {
            result.order[index] = member;
            used[index] = true;
        }
        bucket_index += 1;
    }
    let mut free = 0;
    while bucket_index < buckets.len() && !buckets[bucket_index].is_empty() {
        while used[free] {
            free = (free + 1) % size;
        }
        let member = buckets[bucket_index][0];
        result.map[slot(&keys[member], DEFAULT_SEED)] = -(free as i32) - 1;
        result.order[free] = member;
        used[free] = true;
        free = (free + 1) % size;
        bucket_index += 1;
    }
    Ok(result)
}

fn hash_lookup<K: AsRef<[u8]> + PartialEq>(map: &[i32], ordered: &[K], key: &K) -> Option<usize> {
    if map.is_empty() || map.len() != ordered.len() {
        return None;
    }
    let value = map[toc_hash(key.as_ref(), DEFAULT_SEED) as usize % map.len()];
    let index = if value < 0 {
        (-(value as i64) - 1) as usize
    } else {
        toc_hash(key.as_ref(), value as u32) as usize % map.len()
    };
    (index < ordered.len() && ordered[index] == *key).then_some(index)
}

fn bundle_key(name: &str) -> Vec<u8> {
    name.bytes().map(|byte| byte.to_ascii_lowercase()).collect()
}

/// Bundle names in compressed TOCs: a Huffman tree over 32-bit words of bits.
struct HuffmanNames {
    words: Vec<u32>,
    nodes: Vec<HuffmanNode>,
    root: Option<usize>,
}

struct HuffmanNode {
    value: u32,
    children: Option<(usize, usize)>,
}

impl HuffmanNames {
    fn new(
        data: &[u8],
        names_offset: usize,
        names_count: usize,
        table_offset: usize,
        table_count: usize,
    ) -> Result<Self> {
        let mut reader = Reader::new(data);
        reader.seek(names_offset)?;
        let words = (0..names_count).map(|_| reader.u32(BE)).collect::<Result<Vec<_>>>()?;
        reader.seek(table_offset)?;
        let mut nodes: Vec<HuffmanNode> = Vec::new();
        let mut by_value: HashMap<u32, usize> = HashMap::new();
        let mut left: Option<usize> = None;
        let mut root = None;
        let mut counter: u32 = 0;
        for _ in 0..table_count {
            let value = reader.u32(BE)?;
            let node = *by_value.entry(value).or_insert_with(|| {
                nodes.push(HuffmanNode { value, children: None });
                nodes.len() - 1
            });
            let Some(left_node) = left.take() else {
                left = Some(node);
                continue;
            };
            nodes.push(HuffmanNode { value: counter, children: Some((left_node, node)) });
            let parent = nodes.len() - 1;
            by_value.entry(counter).or_insert(parent);
            root = Some(parent);
            counter += 1;
        }
        if left.is_some() {
            bail!("TOC Huffman tree has an unmatched branch");
        }
        Ok(HuffmanNames { words, nodes, root })
    }

    fn decode(&self, mut bit: usize) -> Result<String> {
        let Some(root) = self.root else { return Ok(String::new()) };
        let bit_count = self.words.len() * 32;
        let mut result = String::new();
        while bit < bit_count {
            let mut node = root;
            while let Some((left, right)) = self.nodes[node].children {
                if bit >= bit_count {
                    bail!("TOC Huffman name exceeds its bitstream");
                }
                let one = (self.words[bit >> 5] >> (bit & 31)) & 1 != 0;
                node = if one { right } else { left };
                bit += 1;
            }
            let character = !self.nodes[node].value as u16;
            if character == 0 {
                return Ok(result);
            }
            if character > 0x7F {
                bail!("TOC Huffman name is not ASCII");
            }
            result.push(character as u8 as char);
        }
        bail!("TOC Huffman name is unterminated")
    }
}

/// Emits a complete patch TOC, envelope included, with uncompressed names.
pub fn write_patch_toc(bundles: &[TocBundle], chunks: &[TocChunk], flags: u32) -> Result<Vec<u8>> {
    if flags & COMPRESSED_NAMES != 0 {
        bail!("patch TOC writer uses uncompressed names");
    }
    let bundle_keys: Vec<Vec<u8>> = bundles.iter().map(|bundle| bundle_key(&bundle.name)).collect();
    let bundle_hash = perfect_hash(&bundle_keys)?;
    let chunk_keys: Vec<[u8; 16]> = chunks.iter().map(|chunk| chunk.guid.0).collect();
    let chunk_hash = perfect_hash(&chunk_keys)?;

    let mut names = Vec::new();
    let mut name_offsets = vec![0u32; bundles.len()];
    for (ordered, &source) in bundle_hash.order.iter().enumerate() {
        if names.len() > u32::MAX as usize {
            bail!("TOC names exceed 4 GiB");
        }
        name_offsets[ordered] = names.len() as u32;
        names.extend_from_slice(bundles[source].name.as_bytes());
        names.push(0);
    }

    let mut chunk_words: Vec<u32> = Vec::new();
    let mut chunk_table = Vec::with_capacity(chunks.len() * 20);
    for &source in &chunk_hash.order {
        let chunk = &chunks[source];
        let mut reversed = chunk.guid.0;
        reversed.reverse();
        chunk_table.extend_from_slice(&reversed);
        if chunk.removed {
            chunk_table.extend_from_slice(&(-1i32).to_be_bytes());
        } else {
            if chunk_words.len() > 0xFF_FFFF {
                bail!("TOC chunk data index exceeds 24 bits");
            }
            chunk_table.extend_from_slice(&((0x80u32 << 24) | chunk_words.len() as u32).to_be_bytes());
            let id = chunk.location.encoded();
            chunk_words.extend_from_slice(&[(id >> 32) as u32, id as u32, chunk.offset, chunk.size]);
        }
    }

    const HEADER_SIZE: usize = 48;
    let mut position = HEADER_SIZE;
    let bundle_hash_offset = position;
    position = align_up(position + bundles.len() * 4, 8);
    let bundle_data_offset = position;
    position = align_up(position + bundles.len() * 16, 4);
    let chunk_hash_offset = position;
    position = align_up(position + chunks.len() * 4, 4);
    let chunk_guid_offset = position;
    position = align_up(position + chunk_table.len(), 4);
    let chunk_data_offset = position;
    position += chunk_words.len() * 4;
    let names_offset = position;
    position = align_up(position + names.len(), 4);
    let mut region_offsets = vec![0u64; bundles.len()];
    for (ordered, &source) in bundle_hash.order.iter().enumerate() {
        region_offsets[ordered] = position as u64;
        position = align_up(position + bundles[source].region.len(), 4);
    }
    if position > u32::MAX as usize {
        bail!("TOC exceeds 4 GiB");
    }

    let mut writer = Writer::new();
    writer.seek(ENVELOPE_SIZE + position);
    writer.seek(0);
    writer.bytes(&MAGIC);
    writer.seek(ENVELOPE_SIZE);
    writer.u32(bundle_hash_offset as u32, BE);
    writer.u32(bundle_data_offset as u32, BE);
    writer.i32(bundles.len() as i32, BE);
    writer.u32(if chunks.is_empty() { 0 } else { chunk_hash_offset as u32 }, BE);
    writer.u32(if chunks.is_empty() { 0 } else { chunk_guid_offset as u32 }, BE);
    writer.i32(chunks.len() as i32, BE);
    writer.u32(chunk_data_offset as u32, BE);
    writer.u32(chunk_data_offset as u32, BE);
    writer.u32(names_offset as u32, BE);
    writer.u32(chunk_data_offset as u32, BE);
    writer.i32(chunk_words.len() as i32, BE);
    writer.u32(flags, BE);
    writer.seek(ENVELOPE_SIZE + bundle_hash_offset);
    for &value in &bundle_hash.map {
        writer.i32(value, BE);
    }
    writer.seek(ENVELOPE_SIZE + bundle_data_offset);
    for (ordered, &source) in bundle_hash.order.iter().enumerate() {
        let bundle = &bundles[source];
        if bundle.region.len() > 0x3FFF_FFFF {
            bail!("TOC bundle region exceeds 30-bit size");
        }
        writer.u32(name_offsets[ordered], BE);
        writer.u32(((bundle.load_flag as u32) << 30) | bundle.region.len() as u32, BE);
        writer.u64(region_offsets[ordered], BE);
    }
    if !chunks.is_empty() {
        writer.seek(ENVELOPE_SIZE + chunk_hash_offset);
        for &value in &chunk_hash.map {
            writer.i32(value, BE);
        }
        writer.seek(ENVELOPE_SIZE + chunk_guid_offset);
        writer.bytes(&chunk_table);
        writer.seek(ENVELOPE_SIZE + chunk_data_offset);
        for &value in &chunk_words {
            writer.u32(value, BE);
        }
    }
    writer.seek(ENVELOPE_SIZE + names_offset);
    writer.bytes(&names);
    for (ordered, &source) in bundle_hash.order.iter().enumerate() {
        writer.seek(ENVELOPE_SIZE + region_offsets[ordered] as usize);
        writer.bytes(&bundles[source].region);
    }
    Ok(writer.into_inner())
}

/// `external_bundle_data` holds regions whose load flag is not 1 (they live in a .sb file).
pub fn read_toc(data: &[u8], external_bundle_data: &[u8]) -> Result<TocDocument> {
    if data.len() < ENVELOPE_SIZE + 48 || data[..4] != MAGIC {
        bail!("TOC envelope is invalid");
    }
    let body = &data[ENVELOPE_SIZE..];
    let mut reader = Reader::new(body);
    let bundle_hash_offset = reader.u32(BE)? as usize;
    let bundle_data_offset = reader.u32(BE)? as usize;
    let bundle_count = reader.i32(BE)?;
    let chunk_hash_offset = reader.u32(BE)? as usize;
    let chunk_guid_offset = reader.u32(BE)? as usize;
    let chunk_count = reader.i32(BE)?;
    reader.u32(BE)?;
    reader.u32(BE)?;
    let names_offset = reader.u32(BE)? as usize;
    let chunk_data_offset = reader.u32(BE)? as usize;
    let data_count = reader.i32(BE)?;
    let flags = reader.u32(BE)?;
    if bundle_count < 0 || chunk_count < 0 || data_count < 0 {
        bail!("TOC has negative table counts");
    }
    let huffman = if flags & COMPRESSED_NAMES != 0 {
        let names_count = reader.u32(BE)? as usize;
        let table_count = reader.u32(BE)? as usize;
        let table_offset = reader.u32(BE)? as usize;
        Some(HuffmanNames::new(body, names_offset, names_count, table_offset, table_count)?)
    } else {
        None
    };
    let (bundle_count, chunk_count) = (bundle_count as usize, chunk_count as usize);
    let mut result = TocDocument { flags, ..Default::default() };
    reader.seek(bundle_hash_offset)?;
    for _ in 0..bundle_count {
        result.bundle_hash_map.push(reader.i32(BE)?);
    }
    reader.seek(bundle_data_offset)?;
    for _ in 0..bundle_count {
        let name_offset = reader.u32(BE)? as usize;
        let size_and_flag = reader.u32(BE)?;
        let region_offset = reader.u64(BE)?;
        let size = (size_and_flag & 0x3FFF_FFFF) as usize;
        let load_flag = (size_and_flag >> 30) as u8;
        let source = if load_flag == 1 { body } else { external_bundle_data };
        if region_offset > source.len() as u64 || size > source.len() - region_offset as usize {
            bail!("TOC bundle region exceeds the file");
        }
        let name = match &huffman {
            Some(huffman) => huffman.decode(name_offset)?,
            None => String::from_utf8_lossy(c_bytes_at(body, names_offset + name_offset, "TOC name")?).into_owned(),
        };
        let begin = region_offset as usize;
        result.bundles.push(TocBundle { name, region: source[begin..begin + size].to_vec(), load_flag: 1 });
    }
    if chunk_count != 0 {
        reader.seek(chunk_hash_offset)?;
        for _ in 0..chunk_count {
            result.chunk_hash_map.push(reader.i32(BE)?);
        }
        reader.seek(chunk_data_offset)?;
        let words = (0..data_count).map(|_| reader.u32(BE)).collect::<Result<Vec<_>>>()?;
        reader.seek(chunk_guid_offset)?;
        for _ in 0..chunk_count {
            let mut guid: [u8; 16] = reader.take(16)?.try_into().expect("16 bytes");
            guid.reverse();
            let encoded_index = reader.i32(BE)?;
            let mut chunk = TocChunk { guid: Guid(guid), ..Default::default() };
            if encoded_index == -1 {
                chunk.removed = true;
            } else {
                let flag = (encoded_index as u32) >> 24;
                let word = (encoded_index as u32 & 0xFF_FFFF) as usize;
                if flag != 0x80 || word > words.len() || words.len() - word < 4 {
                    bail!("TOC chunk data reference is invalid");
                }
                chunk.location = CasIdentifier::decoded(((words[word] as u64) << 32) | words[word + 1] as u64);
                chunk.offset = words[word + 2];
                chunk.size = words[word + 3];
            }
            result.chunks.push(chunk);
        }
    }
    Ok(result)
}

/// Checks that every bundle and chunk resolves through the perfect hashes to its own slot.
pub fn verify_toc(document: &TocDocument) -> Result<()> {
    let bundle_keys: Vec<Vec<u8>> = document.bundles.iter().map(|bundle| bundle_key(&bundle.name)).collect();
    for (index, key) in bundle_keys.iter().enumerate() {
        if hash_lookup(&document.bundle_hash_map, &bundle_keys, key) != Some(index) {
            bail!("TOC bundle perfect hash does not resolve");
        }
    }
    let chunk_keys: Vec<[u8; 16]> = document.chunks.iter().map(|chunk| chunk.guid.0).collect();
    for (index, key) in chunk_keys.iter().enumerate() {
        if hash_lookup(&document.chunk_hash_map, &chunk_keys, key) != Some(index) {
            bail!("TOC chunk perfect hash does not resolve");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_treats_bytes_as_signed() {
        assert_eq!(toc_hash(b"", DEFAULT_SEED), DEFAULT_SEED % FNV_PRIME);
        let high = toc_hash(&[0x80], DEFAULT_SEED);
        let expected = DEFAULT_SEED.wrapping_mul(FNV_PRIME) ^ 0xFFFF_FF80;
        assert_eq!(high, expected % FNV_PRIME);
    }

    #[test]
    fn patch_toc_round_trips_and_resolves() {
        let location = CasIdentifier { patch: true, install_chunk: 2_016_875_820, archive: 1 };
        let bundles = vec![
            TocBundle { name: "Win32/Levels/A".into(), region: vec![1, 2, 3], load_flag: 1 },
            TocBundle { name: "win32/levels/b".into(), region: vec![4; 9], load_flag: 1 },
        ];
        let chunks: Vec<TocChunk> = (0..300)
            .map(|index| TocChunk { guid: Guid::random(), location, offset: index * 10, size: 10, removed: index == 7 })
            .collect();
        let bytes = write_patch_toc(&bundles, &chunks, 3).unwrap();
        let toc = read_toc(&bytes, &[]).unwrap();
        verify_toc(&toc).unwrap();
        assert_eq!(toc.bundles.len(), 2);
        let a = toc.bundles.iter().find(|bundle| bundle.name == "Win32/Levels/A").unwrap();
        assert_eq!(a.region, vec![1, 2, 3]);
        for chunk in &chunks {
            let found = toc.chunks.iter().find(|other| other.guid == chunk.guid).unwrap();
            assert_eq!(found.removed, chunk.removed);
            if !chunk.removed {
                assert_eq!((found.location, found.offset, found.size), (location, chunk.offset, chunk.size));
            }
        }
    }

    #[test]
    fn duplicate_keys_are_refused() {
        let bundles = vec![
            TocBundle { name: "a".into(), region: vec![], load_flag: 1 },
            TocBundle { name: "A".into(), region: vec![], load_flag: 1 },
        ];
        assert!(write_patch_toc(&bundles, &[], 3).is_err());
    }
}
