// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-FileCopyrightText: 2026 the ReSkate contributors
// SPDX-License-Identifier: GPL-3.0-only
// Ported from the ReSkate project (https://github.com/Dingo-Shenanigans/ReSkate); see NOTICE.md.
//! Bounded big/little-endian reader and writer for the native package formats.

use std::fmt;

use crate::{Error, Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

pub use Endian::{Big as BE, Little as LE};

/// A GUID in its native (little-endian, Windows `GUID` memory) byte order.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// A fresh random GUID, laid out like `CoCreateGuid` writes one.
    pub fn random() -> Self {
        Guid(uuid::Uuid::new_v4().to_bytes_le())
    }
}

impl fmt::Display for Guid {
    /// Canonical text order swaps the first three fields, matching Studio.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        let canonical =
            [b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]];
        for (index, byte) in canonical.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Sha1(pub [u8; 20]);

impl Sha1 {
    pub fn of(bytes: &[u8]) -> Self {
        use sha1::Digest;
        Sha1(sha1::Sha1::digest(bytes).into())
    }

    pub fn hex(&self) -> String {
        hex::encode(self.0)
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, position: 0 }
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    pub fn seek(&mut self, position: usize) -> Result<()> {
        if position > self.data.len() {
            bail!("Native reader seek is out of range");
        }
        self.position = position;
        Ok(())
    }

    pub fn skip(&mut self, amount: usize) -> Result<()> {
        let target =
            self.position.checked_add(amount).ok_or_else(|| Error::msg("Native reader seek is out of range"))?;
        self.seek(target)
    }

    pub fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.remaining() {
            bail!("Native reader ran past the end");
        }
        let result = &self.data[self.position..self.position + length];
        self.position += length;
        Ok(result)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("take returns N bytes"))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self, endian: Endian) -> Result<u16> {
        let raw = self.array()?;
        Ok(if endian == BE { u16::from_be_bytes(raw) } else { u16::from_le_bytes(raw) })
    }

    pub fn i16(&mut self, endian: Endian) -> Result<i16> {
        Ok(self.u16(endian)? as i16)
    }

    pub fn u32(&mut self, endian: Endian) -> Result<u32> {
        let raw = self.array()?;
        Ok(if endian == BE { u32::from_be_bytes(raw) } else { u32::from_le_bytes(raw) })
    }

    pub fn i32(&mut self, endian: Endian) -> Result<i32> {
        Ok(self.u32(endian)? as i32)
    }

    pub fn u64(&mut self, endian: Endian) -> Result<u64> {
        let raw = self.array()?;
        Ok(if endian == BE { u64::from_be_bytes(raw) } else { u64::from_le_bytes(raw) })
    }

    pub fn i64(&mut self, endian: Endian) -> Result<i64> {
        Ok(self.u64(endian)? as i64)
    }

    pub fn f32(&mut self, endian: Endian) -> Result<f32> {
        Ok(f32::from_bits(self.u32(endian)?))
    }

    pub fn f64(&mut self, endian: Endian) -> Result<f64> {
        Ok(f64::from_bits(self.u64(endian)?))
    }

    /// Stops at the first NUL but always consumes the whole field.
    pub fn fixed_bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        let raw = self.take(length)?;
        let end = raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len());
        Ok(raw[..end].to_vec())
    }

    /// The bytes up to the next NUL, which is consumed too.
    pub fn c_bytes(&mut self) -> Result<Vec<u8>> {
        let rest = &self.data[self.position..];
        let Some(end) = rest.iter().position(|&byte| byte == 0) else {
            self.position = self.data.len();
            bail!("Native reader ran past the end");
        };
        self.position += end + 1;
        Ok(rest[..end].to_vec())
    }

    pub fn sha1(&mut self) -> Result<Sha1> {
        Ok(Sha1(self.array()?))
    }

    /// The native little-endian form is stored as is; the big-endian form is fully reversed.
    pub fn guid(&mut self, endian: Endian) -> Result<Guid> {
        let mut raw: [u8; 16] = self.array()?;
        if endian == BE {
            raw.reverse();
        }
        Ok(Guid(raw))
    }
}

/// The NUL-terminated string at `offset`, as raw bytes.
pub fn c_bytes_at<'a>(data: &'a [u8], offset: usize, what: &str) -> Result<&'a [u8]> {
    if offset >= data.len() {
        bail!("{what} offset is invalid");
    }
    let rest = &data[offset..];
    match rest.iter().position(|&byte| byte == 0) {
        Some(end) => Ok(&rest[..end]),
        None => bail!("{what} is unterminated"),
    }
}

#[derive(Default)]
pub struct Writer {
    data: Vec<u8>,
    position: usize,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }

    /// Seeking past the end grows the buffer with zeros.
    pub fn seek(&mut self, position: usize) {
        if position > self.data.len() {
            self.data.resize(position, 0);
        }
        self.position = position;
    }

    pub fn bytes(&mut self, value: &[u8]) {
        if value.is_empty() {
            return;
        }
        let end = self.position + value.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[self.position..end].copy_from_slice(value);
        self.position = end;
    }

    pub fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    pub fn u16(&mut self, value: u16, endian: Endian) {
        self.bytes(&if endian == BE { value.to_be_bytes() } else { value.to_le_bytes() });
    }

    pub fn u32(&mut self, value: u32, endian: Endian) {
        self.bytes(&if endian == BE { value.to_be_bytes() } else { value.to_le_bytes() });
    }

    pub fn i32(&mut self, value: i32, endian: Endian) {
        self.u32(value as u32, endian);
    }

    pub fn u64(&mut self, value: u64, endian: Endian) {
        self.bytes(&if endian == BE { value.to_be_bytes() } else { value.to_le_bytes() });
    }

    pub fn sha1(&mut self, value: &Sha1) {
        self.bytes(&value.0);
    }

    /// Mirrors the reader so a value read back writes to the same bytes.
    pub fn guid(&mut self, value: &Guid, endian: Endian) {
        let mut raw = value.0;
        if endian == BE {
            raw.reverse();
        }
        self.bytes(&raw);
    }
}

pub fn align_up(value: usize, alignment: usize) -> usize {
    if alignment <= 1 {
        return value;
    }
    value.div_ceil(alignment) * alignment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_text_swaps_the_first_three_fields() {
        let guid = Guid([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        assert_eq!(guid.to_string(), "03020100-0504-0706-0809-0a0b0c0d0e0f");
    }

    #[test]
    fn reader_and_writer_mirror_each_other() {
        let mut writer = Writer::new();
        writer.u32(0x01020304, BE);
        writer.u16(0x0506, LE);
        let guid = Guid::random();
        writer.guid(&guid, BE);
        let bytes = writer.into_inner();
        assert_eq!(&bytes[..6], &[1, 2, 3, 4, 6, 5]);
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u32(BE).unwrap(), 0x01020304);
        assert_eq!(reader.u16(LE).unwrap(), 0x0506);
        assert_eq!(reader.guid(BE).unwrap(), guid);
        assert!(reader.u8().is_err());
    }
}
