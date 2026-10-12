// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-FileCopyrightText: 2026 the ReSkate contributors
// SPDX-License-Identifier: GPL-3.0-only
// Ported from the ReSkate project (https://github.com/Dingo-Shenanigans/ReSkate); see NOTICE.md.
//! A bundle's region (where each asset payload lives in cas) and its binary manifest.

use super::binary::{BE, Guid, LE, Reader, Sha1, Writer, c_bytes_at};
use crate::{Result, bail};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CasIdentifier {
    pub patch: bool,
    pub install_chunk: u32,
    pub archive: u16,
}

impl CasIdentifier {
    pub fn encoded(&self) -> u64 {
        ((self.patch as u64) << 48) | ((self.install_chunk as u64) << 16) | self.archive as u64
    }

    pub fn decoded(value: u64) -> Self {
        CasIdentifier {
            patch: value & (0xFF << 48) != 0,
            install_chunk: ((value >> 16) & 0xFFFF_FFFF) as u32,
            archive: (value & 0xFFFF) as u16,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BundleFileInfo {
    pub location: CasIdentifier,
    pub offset: u32,
    pub size: u32,
}

/// Where every asset payload lives in cas, plus the inline manifest describing the assets.
#[derive(Clone, Debug, Default)]
pub struct BundleRegion {
    pub files: Vec<BundleFileInfo>,
    pub inline_manifest: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AssetKind {
    #[default]
    Ebx,
    Resource,
    Chunk,
}

#[derive(Clone, Debug, Default)]
pub struct BundleAsset {
    pub kind: AssetKind,
    /// Raw bytes: written back exactly as read.
    pub name: Vec<u8>,
    pub sha1: Sha1,
    pub guid: Guid,
    pub resource_id: u64,
    pub resource_type: u32,
    pub resource_meta: Vec<u8>,
    pub original_size: u64,
    pub logical_offset: u32,
    pub logical_size: u32,
}

#[derive(Clone, Debug, Default)]
pub struct BinaryBundle {
    pub ebx: Vec<BundleAsset>,
    pub resources: Vec<BundleAsset>,
    pub chunks: Vec<BundleAsset>,
    pub chunk_metadata: Vec<u8>,
}

const SALT: u32 = 0x7065_636E;
const STANDARD_MAGIC: u32 = 0xED1C_EDB8;

pub fn read_bundle_region(data: &[u8]) -> Result<BundleRegion> {
    if data.len() < 0x24 {
        bail!("Bundle region header is truncated");
    }
    let mut reader = Reader::new(data);
    let metadata_offset = reader.i32(BE)?;
    let metadata_size = reader.i32(BE)?;
    let location_offset = reader.u32(BE)? as usize;
    let total = reader.i32(BE)?;
    let data_offset = reader.u32(BE)? as usize;
    let resource_offset = reader.u32(BE)? as usize;
    let chunk_offset = reader.u32(BE)? as usize;
    reader.u32(BE)?;
    let repeated_total = reader.i32(BE)?;
    if metadata_offset < 0
        || metadata_size < 0
        || total < 0
        || total != repeated_total
        || resource_offset != data_offset
        || chunk_offset != data_offset
    {
        bail!("Bundle region header is invalid");
    }
    let count = total as usize;
    if location_offset > data.len() || count > data.len() - location_offset || data_offset > data.len() {
        bail!("Bundle region tables exceed the region");
    }
    if (metadata_offset == 0) != (metadata_size == 0) {
        bail!("Bundle region inline metadata range is invalid");
    }

    let mut result = BundleRegion::default();
    if metadata_size != 0 {
        let begin = metadata_offset as usize;
        let size = metadata_size as usize;
        if begin > data.len() || size > data.len() - begin {
            bail!("Bundle inline manifest exceeds the region");
        }
        result.inline_manifest = data[begin..begin + size].to_vec();
    }

    let mut files = Reader::new(&data[data_offset..]);
    let mut current: Option<CasIdentifier> = None;
    result.files.reserve(count);
    for &flag in &data[location_offset..location_offset + count] {
        if flag == 1 {
            let encoded = files.u32(BE)?;
            current = Some(CasIdentifier {
                patch: encoded & 0x00FF_0000 != 0,
                install_chunk: (encoded >> 8) & 0xFF,
                archive: (encoded & 0xFF) as u16,
            });
        } else if flag != 0 {
            current = Some(CasIdentifier::decoded(files.u64(BE)?));
        }
        let Some(location) = current else { bail!("First bundle file has no CAS identifier") };
        let offset = files.u32(BE)?;
        let size = files.u32(BE)?;
        result.files.push(BundleFileInfo { location, offset, size });
    }
    Ok(result)
}

pub fn write_bundle_region(files: &[BundleFileInfo], inline_manifest: &[u8]) -> Result<Vec<u8>> {
    let mut table = Writer::new();
    let mut flags = Vec::with_capacity(files.len());
    let mut previous: Option<CasIdentifier> = None;
    for file in files {
        if previous == Some(file.location) {
            flags.push(0u8);
        } else {
            flags.push(0x80);
            table.u64(file.location.encoded(), BE);
            previous = Some(file.location);
        }
        table.u32(file.offset, BE);
        table.u32(file.size, BE);
    }
    if let Some(first) = flags.first_mut() {
        *first |= 0x04;
    }

    const DATA_OFFSET: usize = 0x24;
    let table_end = DATA_OFFSET + table.data().len();
    let metadata_offset = if inline_manifest.is_empty() { 0 } else { table_end };
    let location_offset = if inline_manifest.is_empty() { table_end } else { metadata_offset + inline_manifest.len() };
    let total_size = location_offset + flags.len();
    if total_size > u32::MAX as usize || files.len() > i32::MAX as usize {
        bail!("Bundle region exceeds native limits");
    }

    let mut output = Writer::new();
    output.seek(total_size);
    output.seek(0);
    output.i32(metadata_offset as i32, BE);
    output.i32(inline_manifest.len() as i32, BE);
    output.u32(location_offset as u32, BE);
    output.i32(files.len() as i32, BE);
    output.u32(DATA_OFFSET as u32, BE);
    output.u32(DATA_OFFSET as u32, BE);
    output.u32(DATA_OFFSET as u32, BE);
    output.u32(0, BE);
    output.i32(files.len() as i32, BE);
    output.seek(DATA_OFFSET);
    output.bytes(table.data());
    output.bytes(inline_manifest);
    output.bytes(&flags);
    Ok(output.into_inner())
}

pub fn read_binary_bundle(data: &[u8]) -> Result<BinaryBundle> {
    let mut reader = Reader::new(data);
    if reader.remaining() < 36 {
        bail!("Binary bundle header is truncated");
    }
    let declared = reader.u32(BE)? as usize;
    if declared != data.len() - 4 {
        bail!("Binary bundle size does not match its stream");
    }
    let mut endian = BE;
    let mut magic = reader.u32(BE)? ^ SALT;
    if magic != STANDARD_MAGIC {
        reader.seek(4)?;
        magic = reader.u32(LE)? ^ SALT;
        endian = LE;
    }
    if magic != STANDARD_MAGIC {
        bail!("Binary bundle magic is unsupported");
    }
    let total = reader.u32(endian)?;
    let ebx_count = reader.u32(endian)? as usize;
    let resource_count = reader.u32(endian)? as usize;
    let chunk_count = reader.u32(endian)? as usize;
    if ebx_count as u64 + resource_count as u64 + chunk_count as u64 != total as u64 {
        bail!("Binary bundle asset counts do not add up");
    }
    let strings = reader.u32(endian)? as usize + 4;
    let metadata_offset = reader.u32(endian)? as usize;
    let metadata_size = reader.u32(endian)? as usize;
    if strings > data.len() {
        bail!("Binary bundle string table is out of range");
    }
    if (total as usize).saturating_mul(20) > reader.remaining() {
        bail!("Native reader ran past the end");
    }

    let hashes = (0..total).map(|_| reader.sha1()).collect::<Result<Vec<_>>>()?;
    let name = |offset: u32| -> Result<Vec<u8>> {
        Ok(c_bytes_at(data, strings + offset as usize, "Binary bundle string")?.to_vec())
    };
    let mut result = BinaryBundle::default();
    for &sha1 in &hashes[..ebx_count] {
        let name_offset = reader.u32(endian)?;
        let original_size = reader.u32(endian)? as u64;
        result.ebx.push(BundleAsset {
            kind: AssetKind::Ebx,
            sha1,
            name: name(name_offset)?,
            original_size,
            ..Default::default()
        });
    }
    for &sha1 in &hashes[ebx_count..ebx_count + resource_count] {
        let name_offset = reader.u32(endian)?;
        let original_size = reader.u32(endian)? as u64;
        result.resources.push(BundleAsset {
            kind: AssetKind::Resource,
            sha1,
            name: name(name_offset)?,
            original_size,
            ..Default::default()
        });
    }
    for asset in &mut result.resources {
        asset.resource_type = reader.u32(endian)?;
    }
    for asset in &mut result.resources {
        asset.resource_meta = reader.take(16)?.to_vec();
    }
    for asset in &mut result.resources {
        asset.resource_id = reader.u64(endian)?;
    }
    for &sha1 in &hashes[ebx_count + resource_count..] {
        let guid = reader.guid(endian)?;
        let logical_offset = reader.u32(endian)?;
        let logical_size = reader.u32(endian)?;
        result.chunks.push(BundleAsset {
            kind: AssetKind::Chunk,
            sha1,
            name: guid.to_string().into_bytes(),
            guid,
            logical_offset,
            logical_size,
            original_size: ((logical_offset & 0xFFFF) | logical_size) as u64,
            ..Default::default()
        });
    }
    if metadata_size != 0 {
        let start = 4 + metadata_offset;
        if start > data.len() || metadata_size > data.len() - start {
            bail!("Binary bundle metadata is out of range");
        }
        result.chunk_metadata = data[start..start + metadata_size].to_vec();
    }
    Ok(result)
}

/// Always writes the standard (big-endian magic, little-endian body) form.
pub fn write_binary_bundle(bundle: &BinaryBundle) -> Result<Vec<u8>> {
    let total = bundle.ebx.len() + bundle.resources.len() + bundle.chunks.len();
    if total > u32::MAX as usize {
        bail!("Binary bundle has too many assets");
    }
    let mut strings = Vec::new();
    let mut name_offsets = Vec::with_capacity(bundle.ebx.len() + bundle.resources.len());
    for asset in bundle.ebx.iter().chain(&bundle.resources) {
        name_offsets.push(strings.len() as u32);
        strings.extend_from_slice(&asset.name);
        strings.push(0);
    }

    const SHA_START: usize = 36;
    let ebx_start = SHA_START + total * 20;
    let resource_start = ebx_start + bundle.ebx.len() * 8;
    let resource_type_start = resource_start + bundle.resources.len() * 8;
    let resource_meta_start = resource_type_start + bundle.resources.len() * 4;
    let resource_id_start = resource_meta_start + bundle.resources.len() * 16;
    let chunk_start = resource_id_start + bundle.resources.len() * 8;
    let string_start = chunk_start + bundle.chunks.len() * 24;
    let metadata_start = string_start + strings.len();
    let length = metadata_start + bundle.chunk_metadata.len();
    if length > u32::MAX as usize {
        bail!("Binary bundle exceeds 4 GiB");
    }

    let mut writer = Writer::new();
    writer.seek(length);
    writer.seek(0);
    writer.u32((length - 4) as u32, BE);
    writer.u32(STANDARD_MAGIC ^ SALT, LE);
    writer.u32(total as u32, LE);
    writer.u32(bundle.ebx.len() as u32, LE);
    writer.u32(bundle.resources.len() as u32, LE);
    writer.u32(bundle.chunks.len() as u32, LE);
    writer.u32((string_start - 4) as u32, LE);
    writer.u32(if bundle.chunk_metadata.is_empty() { 0 } else { (metadata_start - 4) as u32 }, LE);
    writer.u32(bundle.chunk_metadata.len() as u32, LE);
    for asset in bundle.ebx.iter().chain(&bundle.resources).chain(&bundle.chunks) {
        writer.sha1(&asset.sha1);
    }
    for (asset, &offset) in bundle.ebx.iter().chain(&bundle.resources).zip(&name_offsets) {
        writer.u32(offset, LE);
        writer.u32(asset.original_size as u32, LE);
    }
    for asset in &bundle.resources {
        writer.u32(asset.resource_type, LE);
    }
    for asset in &bundle.resources {
        if asset.resource_meta.len() != 16 {
            bail!("Binary bundle resource metadata must be 16 bytes");
        }
        writer.bytes(&asset.resource_meta);
    }
    for asset in &bundle.resources {
        writer.u64(asset.resource_id, LE);
    }
    for asset in &bundle.chunks {
        writer.guid(&asset.guid, LE);
        writer.u32(asset.logical_offset, LE);
        writer.u32(asset.logical_size, LE);
    }
    writer.bytes(&strings);
    writer.bytes(&bundle.chunk_metadata);
    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(archive: u16) -> CasIdentifier {
        CasIdentifier { patch: true, install_chunk: 2_016_875_820, archive }
    }

    #[test]
    fn region_round_trips_with_and_without_a_manifest() {
        let files = [
            BundleFileInfo { location: location(1), offset: 0, size: 10 },
            BundleFileInfo { location: location(1), offset: 10, size: 20 },
            BundleFileInfo { location: location(2), offset: 0, size: 30 },
        ];
        for manifest in [&[][..], &[1, 2, 3][..]] {
            let bytes = write_bundle_region(&files, manifest).unwrap();
            let region = read_bundle_region(&bytes).unwrap();
            assert_eq!(region.files, files);
            assert_eq!(region.inline_manifest, manifest);
        }
    }

    #[test]
    fn binary_bundle_round_trips() {
        let bundle = BinaryBundle {
            ebx: vec![BundleAsset {
                name: b"a/b".to_vec(),
                original_size: 5,
                sha1: Sha1([1; 20]),
                ..Default::default()
            }],
            resources: vec![BundleAsset {
                kind: AssetKind::Resource,
                name: b"a/b".to_vec(),
                resource_type: 0xb2c4_65f6,
                resource_meta: vec![7; 16],
                resource_id: 99,
                original_size: 6,
                ..Default::default()
            }],
            chunks: vec![BundleAsset {
                kind: AssetKind::Chunk,
                guid: Guid([3; 16]),
                logical_size: 40,
                ..Default::default()
            }],
            chunk_metadata: vec![9, 9],
        };
        let bytes = write_binary_bundle(&bundle).unwrap();
        let back = read_binary_bundle(&bytes).unwrap();
        assert_eq!(back.ebx[0].name, b"a/b");
        assert_eq!(back.ebx[0].sha1, Sha1([1; 20]));
        assert_eq!(back.resources[0].resource_id, 99);
        assert_eq!(back.resources[0].resource_meta, vec![7; 16]);
        assert_eq!(back.chunks[0].guid, Guid([3; 16]));
        assert_eq!(back.chunk_metadata, vec![9, 9]);
        assert_eq!(write_binary_bundle(&back).unwrap(), bytes);
    }
}
