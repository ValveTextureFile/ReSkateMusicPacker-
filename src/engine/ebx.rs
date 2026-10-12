// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-FileCopyrightText: 2026 the ReSkate contributors
// SPDX-License-Identifier: GPL-3.0-only
// Ported from the ReSkate project (https://github.com/Dingo-Shenanigans/ReSkate); see NOTICE.md.
//! EBX partitions (RIFF: EBXD data, EFIX fixups, EBXX arrays/boxed values, REFL/RFL2 reflection).
//!
//! `read_document` parses a partition into owned objects; `write_document` lays it out again
//! with the donor's reflection table, so a template asset can be cloned and edited.

use std::collections::HashMap;

use super::binary::{BE, Guid, LE, Reader, Sha1, Writer, align_up};
use crate::{Error, Result, bail};

/// Field and type kinds, from bits 4..9 of an encoded type.
pub mod kind {
    pub const INHERITED: u8 = 0x00;
    pub const STRUCTURE: u8 = 0x02;
    pub const POINTER: u8 = 0x03;
    pub const STRING: u8 = 0x06;
    pub const CSTRING: u8 = 0x07;
    pub const ENUMERATION: u8 = 0x08;
    pub const FILE_REF: u8 = 0x09;
    pub const BOOLEAN: u8 = 0x0A;
    pub const INT8: u8 = 0x0B;
    pub const UINT8: u8 = 0x0C;
    pub const INT16: u8 = 0x0D;
    pub const UINT16: u8 = 0x0E;
    pub const INT32: u8 = 0x0F;
    pub const UINT32: u8 = 0x10;
    pub const UINT64: u8 = 0x11;
    pub const INT64: u8 = 0x12;
    pub const FLOAT32: u8 = 0x13;
    pub const FLOAT64: u8 = 0x14;
    pub const GUID: u8 = 0x15;
    pub const SHA1: u8 = 0x16;
    pub const RESOURCE_REF: u8 = 0x17;
    pub const TYPE_REF: u8 = 0x19;
    pub const BOXED_VALUE_REF: u8 = 0x1A;
    pub const DELEGATE: u8 = 0x1C;

    /// The low nibble of an encoded type.
    pub const CATEGORY_ARRAY: u8 = 4;
}

#[derive(Clone, Debug, Default)]
pub struct FieldDescriptor {
    pub name: String,
    pub name_hash: u32,
    pub encoded_type: u16,
    pub class_ref: u16,
    pub data_offset: u32,
}

impl FieldDescriptor {
    pub fn kind(&self) -> u8 {
        ((self.encoded_type >> 4) & 0x1F) as u8
    }

    pub fn is_array(&self) -> bool {
        (self.encoded_type & 0x0F) as u8 == kind::CATEGORY_ARRAY
    }
}

#[derive(Clone, Debug, Default)]
pub struct TypeDescriptor {
    pub name: String,
    pub name_hash: u32,
    pub field_index: i32,
    pub field_count: u16,
    pub encoded_type: u16,
    pub size: u16,
    pub alignment: u16,
    pub guid: Guid,
    pub signature: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportReference {
    pub file_guid: Guid,
    pub class_guid: Guid,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ArrayRecord {
    pub offset: u32,
    pub count: u32,
    pub hash: u32,
    pub encoded_type: u16,
    pub class_ref: u16,
}

#[derive(Clone, Debug, Default)]
pub struct BoxedValueRecord {
    pub offset: u32,
    pub count: u32,
    pub hash: u32,
    pub encoded_type: u16,
    pub class_ref: u16,
    pub raw_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct InstanceRecord {
    pub fixup_type: u16,
    pub descriptor: i32,
    pub data_offset: u32,
    pub exported: bool,
    pub instance_guid: Guid,
    pub object: Option<Object>,
    /// The instance's bytes as read; the writer starts from them so unknown fields survive.
    pub raw_image: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerKind {
    #[default]
    Null,
    Internal,
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerReference {
    pub kind: PointerKind,
    /// An instance index (internal) or import index (external).
    pub index: i32,
}

impl Default for PointerReference {
    fn default() -> Self {
        PointerReference { kind: PointerKind::Null, index: -1 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypeReference {
    pub primitive: bool,
    pub primitive_kind: u8,
    pub descriptor: i32,
    pub encoded: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoxedReference {
    pub encoded_type: u32,
    pub data_offset: i64,
    pub relative_offset: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    None,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    /// String and file-reference fields, as raw bytes.
    Bytes(Vec<u8>),
    Guid(Guid),
    Sha1(Sha1),
    Resource(u64),
    Pointer(PointerReference),
    TypeRef(TypeReference),
    Boxed(BoxedReference),
    Object(Box<Object>),
    Array(Vec<Value>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldValue {
    pub descriptor: usize,
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object {
    pub descriptor: i32,
    /// Inherited fields first, flattened, in layout order.
    pub fields: Vec<FieldValue>,
}

impl Object {
    /// The field named `name` (exact match).
    pub fn field_mut(&mut self, name: &str) -> Result<&mut Value> {
        self.fields
            .iter_mut()
            .find(|field| field.name == name)
            .map(|field| &mut field.value)
            .ok_or_else(|| Error::msg(format!("No field {name}")))
    }

    pub fn field(&self, name: &str) -> Result<&Value> {
        self.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
            .ok_or_else(|| Error::msg(format!("No field {name}")))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub file_guid: Guid,
    pub root_type: String,
    pub data_start: usize,
    pub data_end: usize,
    pub arrays_offset: u32,
    pub boxed_values_offset: u32,
    pub strings_offset: u32,
    pub serialized_layout: bool,
    pub hashed_reflection_names: bool,
    pub reflection_data: Vec<u8>,
    pub fixup_type_guids: Vec<Guid>,
    pub fixup_type_signatures: Vec<u32>,
    pub boxed_reference_offsets: Vec<u32>,
    pub types: Vec<TypeDescriptor>,
    pub fields: Vec<FieldDescriptor>,
    pub instances: Vec<InstanceRecord>,
    pub imports: Vec<ImportReference>,
    pub arrays: Vec<ArrayRecord>,
    pub boxed_values: Vec<BoxedValueRecord>,
    pub pointer_offsets: Vec<u32>,
    pub resource_ref_offsets: Vec<u32>,
    pub import_offsets: Vec<u32>,
    pub type_info_offsets: Vec<u32>,
    pub boxed_value_slots: Vec<u32>,
}

impl Document {
    /// The first instance of the named type that has an object.
    pub fn instance_of(&mut self, type_name: &str) -> Result<&mut InstanceRecord> {
        let types = &self.types;
        self.instances
            .iter_mut()
            .find(|instance| {
                instance.object.is_some()
                    && usize::try_from(instance.descriptor)
                        .ok()
                        .and_then(|index| types.get(index))
                        .is_some_and(|ty| ty.name == type_name)
            })
            .ok_or_else(|| Error::msg(format!("No {type_name} instance")))
    }

    /// The object of the first instance of the named type.
    pub fn object_of(&mut self, type_name: &str) -> Result<&mut Object> {
        Ok(self.instance_of(type_name)?.object.as_mut().expect("instance_of only returns instances with objects"))
    }
}

// ---- reading -------------------------------------------------------------------------------------

const RIFF: u32 = 0x4646_4952; // "RIFF" read little-endian
const FORM_EBX: u32 = 0x4542_5800;
const FORM_EBXS: u32 = 0x4542_5853;
const EBXD: u32 = 0x4542_5844;
const EFIX: u32 = 0x4546_4958;
const EBXX: u32 = 0x4542_5858;
const REFL: u32 = 0x5245_464C;
const RFL2: u32 = 0x5246_4C32;

#[derive(Clone, Copy)]
struct Chunk {
    id: u32,
    begin: usize,
    end: usize,
}

fn read_chunks(reader: &mut Reader<'_>) -> Result<Vec<Chunk>> {
    if reader.u32(LE)? != RIFF {
        bail!("EBX is not a RIFF document");
    }
    let riff_end = reader.u32(LE)? as usize + 8;
    if riff_end != reader.len() {
        bail!("RIFF EBX size does not match its stream");
    }
    let form = reader.u32(BE)?;
    if form != FORM_EBX && form != FORM_EBXS {
        bail!("RIFF document is not EBX or EBXS");
    }
    let mut chunks = Vec::new();
    while reader.position() < riff_end {
        if reader.remaining() < 8 {
            bail!("truncated RIFF chunk header");
        }
        let id = reader.u32(BE)?;
        let size = reader.u32(LE)? as usize;
        let begin = reader.position();
        if size > riff_end - begin {
            bail!("RIFF chunk exceeds its document");
        }
        let end = begin + size;
        chunks.push(Chunk { id, begin, end });
        let next = end + (size & 1);
        if next > riff_end {
            bail!("RIFF chunk padding is truncated");
        }
        reader.seek(next)?;
    }
    Ok(chunks)
}

fn find_chunk(chunks: &[Chunk], id: u32, name: &str) -> Result<Option<Chunk>> {
    let mut found = chunks.iter().filter(|chunk| chunk.id == id);
    let first = found.next().copied();
    if found.next().is_some() {
        bail!("duplicate RIFF {name} chunk");
    }
    Ok(first)
}

fn unique_chunk(chunks: &[Chunk], id: u32, name: &str) -> Result<Chunk> {
    find_chunk(chunks, id, name)?.ok_or_else(|| Error::msg(format!("missing RIFF {name} chunk")))
}

fn count(reader: &mut Reader<'_>, label: &str) -> Result<usize> {
    let value = reader.i32(LE)?;
    if value < 0 {
        bail!("{label} count cannot be negative");
    }
    Ok(value as usize)
}

fn ensure_records(reader: &Reader<'_>, end: usize, count: usize, stride: usize, label: &str) -> Result<()> {
    if reader.position() > end || count > (end - reader.position()) / stride {
        bail!("{label} records exceed their chunk");
    }
    Ok(())
}

fn validate_data_offset(document: &Document, offset: u32, required: usize, label: &str) -> Result<()> {
    let data_size = document.data_end - document.data_start;
    let offset = offset as usize;
    if offset > data_size || required > data_size - offset {
        bail!("{label} offset exceeds EBXD");
    }
    Ok(())
}

fn read_fixup(reader: &mut Reader<'_>, chunk: Chunk, document: &mut Document) -> Result<()> {
    reader.seek(chunk.begin)?;
    document.file_guid = reader.guid(LE)?;
    let type_count = count(reader, "EFIX type")?;
    ensure_records(reader, chunk.end, type_count, 16, "EFIX type")?;
    document.fixup_type_guids = (0..type_count).map(|_| reader.guid(LE)).collect::<Result<_>>()?;
    let signature_count = count(reader, "EFIX signature")?;
    if signature_count > type_count {
        bail!("EFIX has more signatures than types");
    }
    ensure_records(reader, chunk.end, signature_count, 4, "EFIX signature")?;
    document.fixup_type_signatures = (0..signature_count).map(|_| reader.u32(LE)).collect::<Result<_>>()?;

    let exported_count = count(reader, "EFIX exported instance")?;
    let instance_count = count(reader, "EFIX instance")?;
    if exported_count > instance_count {
        bail!("EFIX exported instance count is invalid");
    }
    ensure_records(reader, chunk.end, instance_count, 4, "EFIX instance")?;
    for index in 0..instance_count {
        let offset = reader.u32(LE)?;
        validate_data_offset(document, offset, 2, "EFIX instance")?;
        let saved = reader.position();
        reader.seek(document.data_start + offset as usize)?;
        let class_ref = reader.u16(LE)?;
        reader.seek(saved)?;
        if class_ref as usize >= document.fixup_type_guids.len() {
            bail!("EFIX instance type is invalid");
        }
        document.instances.push(InstanceRecord {
            fixup_type: class_ref,
            descriptor: -1,
            data_offset: offset,
            exported: index < exported_count,
            ..Default::default()
        });
    }

    let offsets = |reader: &mut Reader<'_>, document: &Document, label: &str, width: usize| -> Result<Vec<u32>> {
        let count = count(reader, label)?;
        ensure_records(reader, chunk.end, count, 4, label)?;
        (0..count)
            .map(|_| {
                let offset = reader.u32(LE)?;
                validate_data_offset(document, offset, width, label)?;
                Ok(offset)
            })
            .collect()
    };
    document.pointer_offsets = offsets(reader, document, "EFIX pointer", 8)?;
    document.resource_ref_offsets = offsets(reader, document, "EFIX resource reference", 8)?;

    let import_count = count(reader, "EFIX import")?;
    ensure_records(reader, chunk.end, import_count, 32, "EFIX import")?;
    for _ in 0..import_count {
        let file_guid = reader.guid(LE)?;
        let class_guid = reader.guid(LE)?;
        document.imports.push(ImportReference { file_guid, class_guid });
    }
    document.import_offsets = offsets(reader, document, "EFIX import relocation", 8)?;
    document.type_info_offsets = offsets(reader, document, "EFIX type-info reference", 8)?;
    if chunk.end - reader.position() < 12 {
        bail!("truncated EFIX section offsets");
    }
    document.arrays_offset = reader.u32(LE)?;
    document.boxed_values_offset = reader.u32(LE)?;
    document.strings_offset = reader.u32(LE)?;
    validate_data_offset(document, document.arrays_offset, 0, "EFIX array section")?;
    validate_data_offset(document, document.boxed_values_offset, 0, "EFIX boxed-value section")?;
    validate_data_offset(document, document.strings_offset, 0, "EFIX string section")?;
    if reader.position() < chunk.end {
        document.boxed_reference_offsets = offsets(reader, document, "EFIX boxed reference", 16)?;
    }
    if reader.position() != chunk.end {
        bail!("unexpected bytes at the end of EFIX");
    }
    Ok(())
}

fn read_extra(reader: &mut Reader<'_>, chunk: Chunk, document: &mut Document) -> Result<()> {
    reader.seek(chunk.begin)?;
    let array_count = count(reader, "EBXX array")?;
    let boxed_count = count(reader, "EBXX boxed value")?;
    ensure_records(reader, chunk.end, array_count + boxed_count, 16, "EBXX")?;
    for _ in 0..array_count {
        let offset = reader.u32(LE)?;
        let count = reader.i32(LE)?;
        if count < 0 {
            bail!("EBXX array has a negative count");
        }
        let record = ArrayRecord {
            offset,
            count: count as u32,
            hash: reader.u32(LE)?,
            encoded_type: reader.u16(LE)?,
            class_ref: reader.u16(LE)?,
        };
        validate_data_offset(document, offset, 0, "EBXX array")?;
        document.arrays.push(record);
    }
    for _ in 0..boxed_count {
        let offset = reader.u32(LE)?;
        let count = reader.i32(LE)?;
        if count < 0 {
            bail!("EBXX boxed value has a negative count");
        }
        let record = BoxedValueRecord {
            offset,
            count: count as u32,
            hash: reader.u32(LE)?,
            encoded_type: reader.u16(LE)?,
            class_ref: reader.u16(LE)?,
            raw_bytes: Vec::new(),
        };
        validate_data_offset(document, offset, 0, "EBXX boxed value")?;
        document.boxed_values.push(record);
    }
    if reader.position() != chunk.end {
        bail!("unexpected bytes at the end of EBXX");
    }
    Ok(())
}

fn string_at(data: &[u8], offset: usize, end: usize) -> Result<String> {
    if offset >= end {
        bail!("reflection string offset is invalid");
    }
    match data[offset..end].iter().position(|&byte| byte == 0) {
        Some(length) => Ok(String::from_utf8_lossy(&data[offset..offset + length]).into_owned()),
        None => bail!("reflection string is unterminated"),
    }
}

/// Reads the reflection chunk; returns each type's (GUID, signature).
fn read_reflection(
    bytes: &[u8],
    reader: &mut Reader<'_>,
    chunk: Chunk,
    hashed_names: bool,
    document: &mut Document,
) -> Result<Vec<(Guid, u32)>> {
    reader.seek(chunk.begin)?;
    let signature_count = count(reader, "reflection signature")?;
    ensure_records(reader, chunk.end, signature_count, 20, "reflection signature")?;
    let ids = (0..signature_count).map(|_| Ok((reader.guid(LE)?, reader.u32(LE)?))).collect::<Result<Vec<_>>>()?;

    let type_count = count(reader, "reflection type")?;
    ensure_records(reader, chunk.end, type_count, 16, "reflection type")?;
    for index in 0..type_count {
        let mut value = TypeDescriptor {
            name_hash: reader.u32(LE)?,
            field_index: reader.i32(LE)?,
            field_count: reader.u16(LE)?,
            encoded_type: reader.u16(LE)? >> 1,
            size: reader.u16(LE)?,
            alignment: reader.u16(LE)?,
            ..Default::default()
        };
        if value.alignment > u8::MAX as u16 {
            bail!("reflection type alignment is unsupported");
        }
        if let Some(&(guid, signature)) = ids.get(index) {
            value.guid = guid;
            value.signature = signature;
        }
        document.types.push(value);
    }

    let field_count = count(reader, "reflection field")?;
    ensure_records(reader, chunk.end, field_count, 12, "reflection field")?;
    for _ in 0..field_count {
        document.fields.push(FieldDescriptor {
            name_hash: reader.u32(LE)?,
            data_offset: reader.u32(LE)?,
            encoded_type: reader.u16(LE)? >> 1,
            class_ref: reader.u16(LE)?,
            ..Default::default()
        });
    }
    for ty in &document.types {
        if ty.field_index < 0
            || ty.field_index as usize > document.fields.len()
            || ty.field_count as usize > document.fields.len() - ty.field_index as usize
        {
            bail!("reflection type field range is invalid");
        }
    }

    let group_count = count(reader, "reflection group")?;
    ensure_records(reader, chunk.end, group_count, 12, "reflection group")?;
    reader.skip(group_count * 12)?;
    let mapping_count = count(reader, "reflection mapping")?;
    ensure_records(reader, chunk.end, mapping_count, 8, "reflection mapping")?;
    reader.skip(mapping_count * 8)?;

    if hashed_names {
        let name_count = count(reader, "reflection name")?;
        ensure_records(reader, chunk.end, name_count, 8, "reflection name")?;
        let mut offsets = HashMap::new();
        for _ in 0..name_count {
            let hash = reader.u32(LE)?;
            let offset = reader.u32(LE)?;
            offsets.insert(hash, offset);
        }
        let strings = reader.position();
        let total = offsets.len();
        for ty in &mut document.types {
            let Some(&offset) = offsets.get(&ty.name_hash) else {
                bail!("RFL2 type name 0x{} is missing from {total} names", ty.name_hash);
            };
            ty.name = string_at(bytes, strings + offset as usize, chunk.end)?;
        }
        for field in &mut document.fields {
            let Some(&offset) = offsets.get(&field.name_hash) else { bail!("RFL2 field name is missing") };
            field.name = string_at(bytes, strings + offset as usize, chunk.end)?;
        }
    } else {
        let strings = reader.position();
        for ty in &mut document.types {
            ty.name = string_at(bytes, strings + ty.name_hash as usize, chunk.end)?;
        }
        for field in &mut document.fields {
            field.name = string_at(bytes, strings + field.name_hash as usize, chunk.end)?;
        }
    }
    Ok(ids)
}

fn map_instances(document: &mut Document, reflection: &[(Guid, u32)]) {
    for instance in &mut document.instances {
        let slot = instance.fixup_type as usize;
        let (Some(guid), Some(signature)) =
            (document.fixup_type_guids.get(slot), document.fixup_type_signatures.get(slot))
        else {
            continue;
        };
        if let Some(index) = reflection.iter().position(|(id, sig)| id == guid && sig == signature) {
            instance.descriptor = index as i32;
        }
    }
    if let Some(first) = document.instances.first()
        && let Some(ty) = usize::try_from(first.descriptor).ok().and_then(|index| document.types.get(index))
    {
        document.root_type = ty.name.clone();
    }
}

struct ValueReader<'a> {
    reader: Reader<'a>,
    document: &'a Document,
    depth: usize,
    boxed_value_slots: Vec<u32>,
}

impl ValueReader<'_> {
    fn nest(&mut self) -> Result<()> {
        // A type can name itself as its base or as a member: bound the nesting so a bad file
        // fails to parse instead of overflowing the stack.
        if self.depth >= 128 {
            bail!("EBX types nest too deeply");
        }
        self.depth += 1;
        Ok(())
    }

    fn object(&mut self, descriptor: i32, start: usize) -> Result<Object> {
        self.nest()?;
        let document = self.document;
        let Some(ty) = usize::try_from(descriptor).ok().and_then(|index| document.types.get(index)) else {
            bail!("EBX object type reference is invalid");
        };
        if start < document.data_start || ty.size as usize > document.data_end.saturating_sub(start) {
            bail!("EBX object exceeds EBXD");
        }
        let mut result = Object { descriptor, fields: Vec::new() };
        self.fields(&mut result, ty, start)?;
        self.depth -= 1;
        Ok(result)
    }

    fn fields(&mut self, output: &mut Object, ty: &TypeDescriptor, start: usize) -> Result<()> {
        let document = self.document;
        for ordinal in 0..ty.field_count as usize {
            let descriptor = ty.field_index as usize + ordinal;
            let field = &document.fields[descriptor];
            if field.kind() == kind::INHERITED {
                let Some(base) = document.types.get(field.class_ref as usize) else {
                    bail!("EBX base type is invalid")
                };
                self.nest()?;
                self.fields(output, base, start)?;
                self.depth -= 1;
                continue;
            }
            let position = start + field.data_offset as usize;
            if position > document.data_end {
                bail!("EBX field offset exceeds EBXD");
            }
            let value = self.field(field, position)?;
            output.fields.push(FieldValue { descriptor, name: field.name.clone(), value });
        }
        Ok(())
    }

    fn field(&mut self, field: &FieldDescriptor, position: usize) -> Result<Value> {
        let document = self.document;
        if !field.is_array() {
            self.reader.seek(position)?;
            return self.value(field.kind(), field.class_ref, position);
        }
        self.reader.seek(position)?;
        let displacement = self.reader.i32(LE)? as i64;
        let target = position as i64 + displacement;
        if target < (document.data_start + 4) as i64 || target > document.data_end as i64 {
            bail!("EBX array displacement exceeds EBXD");
        }
        let target = target as usize;
        self.reader.seek(target - 4)?;
        let count = self.reader.i32(LE)?;
        if count < 0 {
            bail!("EBX array has a negative count");
        }
        // Every element takes at least a byte.
        if count as usize > document.data_end - target {
            bail!("EBX array exceeds EBXD");
        }
        self.reader.seek(target)?;
        let mut values = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let element_start = self.reader.position();
            values.push(self.value(field.kind(), field.class_ref, element_start)?);
            match field.kind() {
                kind::STRUCTURE => {
                    let Some(ty) = document.types.get(field.class_ref as usize) else {
                        bail!("EBX array struct type is invalid");
                    };
                    self.reader.seek(element_start + ty.size as usize)?;
                }
                kind::POINTER | kind::CSTRING => {
                    let aligned = align_up(self.reader.position(), 8);
                    self.reader.seek(aligned)?;
                }
                _ => {}
            }
        }
        Ok(Value::Array(values))
    }

    fn value(&mut self, field_kind: u8, class_ref: u16, position: usize) -> Result<Value> {
        let document = self.document;
        let reader = &mut self.reader;
        Ok(match field_kind {
            kind::BOOLEAN => Value::Bool(reader.u8()? != 0),
            kind::INT8 => Value::Int(reader.i8()? as i64),
            kind::UINT8 => Value::UInt(reader.u8()? as u64),
            kind::INT16 => Value::Int(reader.i16(LE)? as i64),
            kind::UINT16 => Value::UInt(reader.u16(LE)? as u64),
            kind::INT32 | kind::ENUMERATION => Value::Int(reader.i32(LE)? as i64),
            kind::UINT32 => Value::UInt(reader.u32(LE)? as u64),
            kind::INT64 => Value::Int(reader.i64(LE)?),
            kind::UINT64 => Value::UInt(reader.u64(LE)?),
            kind::FLOAT32 => Value::Float(reader.f32(LE)? as f64),
            kind::FLOAT64 => Value::Float(reader.f64(LE)?),
            kind::GUID => Value::Guid(reader.guid(LE)?),
            kind::SHA1 => Value::Sha1(reader.sha1()?),
            kind::RESOURCE_REF => Value::Resource(reader.u64(LE)?),
            kind::STRING => Value::Bytes(reader.fixed_bytes(32)?),
            kind::CSTRING => {
                let displacement = reader.u32(LE)?;
                Value::Bytes(self.relative_string(position, displacement)?)
            }
            kind::FILE_REF => {
                let displacement = reader.u32(LE)?;
                reader.skip(4)?;
                Value::Bytes(self.relative_string(position, displacement)?)
            }
            kind::POINTER => {
                let encoded = reader.i32(LE)?;
                if encoded == 0 {
                    Value::Pointer(PointerReference::default())
                } else if encoded & 1 != 0 {
                    let index = encoded >> 1;
                    if index < 0 || index as usize >= document.imports.len() {
                        bail!("EBX pointer import is invalid");
                    }
                    Value::Pointer(PointerReference { kind: PointerKind::External, index })
                } else {
                    let target = position as i64 + encoded as i64 - document.data_start as i64;
                    let Some(index) =
                        document.instances.iter().position(|instance| instance.data_offset as i64 == target)
                    else {
                        bail!("EBX internal pointer target is invalid");
                    };
                    Value::Pointer(PointerReference { kind: PointerKind::Internal, index: index as i32 })
                }
            }
            kind::TYPE_REF | kind::DELEGATE => {
                let encoded = reader.u32(LE)?;
                reader.skip(4)?;
                if encoded == 0 {
                    Value::TypeRef(TypeReference::default())
                } else if encoded & 0x8000_0000 != 0 {
                    let primitive_kind = (((encoded & !0x8000_0000) >> 5) & 0x1F) as u8;
                    Value::TypeRef(TypeReference { primitive: true, primitive_kind, descriptor: -1, encoded })
                } else {
                    Value::TypeRef(TypeReference {
                        primitive: false,
                        primitive_kind: 0,
                        descriptor: (encoded >> 2) as i32,
                        encoded,
                    })
                }
            }
            kind::BOXED_VALUE_REF => {
                let encoded_type = reader.u32(LE)?;
                reader.skip(4)?;
                let displacement = reader.i64(LE)?;
                let data_offset = (position + 8) as i64 + displacement - document.data_start as i64;
                self.boxed_value_slots.push((position - document.data_start) as u32);
                Value::Boxed(BoxedReference { encoded_type, data_offset, relative_offset: displacement })
            }
            kind::STRUCTURE => Value::Object(Box::new(self.object(class_ref as i32, position)?)),
            _ => Value::None,
        })
    }

    fn relative_string(&mut self, base: usize, displacement: u32) -> Result<Vec<u8>> {
        if displacement == u32::MAX {
            return Ok(Vec::new());
        }
        let document = self.document;
        let target = base as u64 + displacement as u64;
        if target < document.data_start as u64 || target >= document.data_end as u64 {
            bail!("EBX string displacement exceeds EBXD");
        }
        let saved = self.reader.position();
        self.reader.seek(target as usize)?;
        let value = self.reader.c_bytes()?;
        if self.reader.position() > document.data_end {
            bail!("EBX string exceeds EBXD");
        }
        self.reader.seek(saved)?;
        Ok(value)
    }
}

pub fn read_document(bytes: &[u8]) -> Result<Document> {
    let mut reader = Reader::new(bytes);
    let chunks = read_chunks(&mut reader)?;
    let data = unique_chunk(&chunks, EBXD, "EBXD")?;
    let fixup = unique_chunk(&chunks, EFIX, "EFIX")?;
    let extra = unique_chunk(&chunks, EBXX, "EBXX")?;
    let reflection = find_chunk(&chunks, REFL, "REFL")?;
    let reflection2 = find_chunk(&chunks, RFL2, "RFL2")?;
    let (reflection, hashed) = match (reflection, reflection2) {
        (Some(chunk), None) => (chunk, false),
        (None, Some(chunk)) => (chunk, true),
        _ => bail!("RIFF EBX must contain exactly one REFL or RFL2 chunk"),
    };

    let mut document = Document::default();
    reader.seek(8)?;
    document.serialized_layout = reader.u32(BE)? == FORM_EBXS;
    document.data_start = align_up(data.begin, 16);
    document.data_end = data.end;
    if document.data_start > document.data_end {
        bail!("EBXD alignment exceeds the chunk");
    }
    read_fixup(&mut reader, fixup, &mut document)?;
    read_extra(&mut reader, extra, &mut document)?;
    let reflection_ids = read_reflection(bytes, &mut reader, reflection, hashed, &mut document)?;
    document.hashed_reflection_names = hashed;
    document.reflection_data = bytes[reflection.begin..reflection.end].to_vec();
    map_instances(&mut document, &reflection_ids);

    // Objects are read against a snapshot of the tables, then attached.
    let mut read = Vec::with_capacity(document.instances.len());
    let mut values =
        ValueReader { reader: Reader::new(bytes), document: &document, depth: 0, boxed_value_slots: Vec::new() };
    for instance in &document.instances {
        let Some(ty) = usize::try_from(instance.descriptor).ok().and_then(|index| document.types.get(index)) else {
            read.push(None);
            continue;
        };
        let class_start = document.data_start + instance.data_offset as usize;
        let mut guid = Guid::default();
        if instance.exported {
            if class_start < document.data_start + 16 {
                bail!("exported EBX instance has no GUID");
            }
            values.reader.seek(class_start - 16)?;
            guid = values.reader.guid(LE)?;
        }
        if ty.size as usize > document.data_end - class_start {
            bail!("EBX object exceeds EBXD");
        }
        let raw_image = bytes[class_start..class_start + ty.size as usize].to_vec();
        let object = values.object(instance.descriptor, class_start)?;
        read.push(Some((guid, raw_image, object)));
    }
    let boxed_value_slots = std::mem::take(&mut values.boxed_value_slots);
    for (instance, read) in document.instances.iter_mut().zip(read) {
        if let Some((guid, raw_image, object)) = read {
            if instance.exported {
                instance.instance_guid = guid;
            }
            instance.raw_image = raw_image;
            instance.object = Some(object);
        }
    }
    document.boxed_value_slots = boxed_value_slots;

    document.boxed_values.sort_by_key(|record| record.offset);
    for index in 0..document.boxed_values.len() {
        let begin = document.boxed_values[index].offset;
        let end = document.boxed_values.get(index + 1).map_or(document.strings_offset, |next| next.offset);
        if end < begin {
            bail!("EBXX boxed values are not ordered");
        }
        validate_data_offset(&document, begin, (end - begin) as usize, "EBXX boxed value bytes")?;
        let start = document.data_start;
        document.boxed_values[index].raw_bytes = bytes[start + begin as usize..start + end as usize].to_vec();
    }
    Ok(document)
}

// ---- writing -------------------------------------------------------------------------------------

fn scalar_size(field_kind: u8) -> usize {
    match field_kind {
        kind::BOOLEAN | kind::INT8 | kind::UINT8 => 1,
        kind::INT16 | kind::UINT16 => 2,
        kind::INT32 | kind::UINT32 | kind::FLOAT32 | kind::ENUMERATION => 4,
        kind::GUID => 16,
        kind::SHA1 => 20,
        kind::STRING => 32,
        kind::BOXED_VALUE_REF => 16,
        _ => 8,
    }
}

fn width_error() -> Error {
    Error::msg("EBX integer field exceeds its native width")
}

/// A value as an integer of the given range, converting the way the C++ tool's `number<T>` did.
fn integer(value: &Value, min: i128, max: i128) -> Result<i128> {
    let number = match value {
        Value::Int(v) => *v as i128,
        Value::UInt(v) => *v as i128,
        Value::Float(v) => {
            if !v.is_finite() {
                bail!("EBX floating-point field must be finite");
            }
            return Ok(*v as i128);
        }
        Value::Bool(v) => return Ok(*v as i128),
        _ => return Ok(0),
    };
    if number < min || number > max {
        return Err(width_error());
    }
    Ok(number)
}

fn float(value: &Value, single: bool) -> Result<f64> {
    Ok(match value {
        Value::Int(v) => *v as f64,
        Value::UInt(v) => *v as f64,
        Value::Float(v) => {
            if !v.is_finite() {
                bail!("EBX floating-point field must be finite");
            }
            if single && v.abs() > f32::MAX as f64 {
                bail!("EBX floating-point field exceeds its native width");
            }
            *v
        }
        Value::Bool(v) => *v as u8 as f64,
        _ => 0.0,
    })
}

fn checked_u32(value: usize, label: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::msg(label.to_owned()))
}

struct PendingArray<'a> {
    slot: u32,
    values: &'a [Value],
    descriptor: usize,
    emitted: bool,
}

struct Builder<'a> {
    document: &'a Document,
    payload: Vec<u8>,
    instance_offsets: Vec<u32>,
    pointer_offsets: Vec<u32>,
    resource_offsets: Vec<u32>,
    import_offsets: Vec<u32>,
    type_info_offsets: Vec<u32>,
    arrays_pending: Vec<PendingArray<'a>>,
    strings_pending: Vec<(u32, &'a [u8])>,
    pointers_pending: Vec<(u32, PointerReference)>,
    boxes_pending: Vec<(u32, BoxedReference)>,
    arrays: Vec<ArrayRecord>,
    boxed: Vec<BoxedValueRecord>,
    arrays_offset: u32,
    boxed_offset: u32,
    empty_array_target: u32,
}

const NO_VALUE: &Value = &Value::None;

impl<'a> Builder<'a> {
    fn new(document: &'a Document) -> Self {
        Builder {
            document,
            payload: Vec::new(),
            instance_offsets: Vec::new(),
            pointer_offsets: Vec::new(),
            resource_offsets: Vec::new(),
            import_offsets: Vec::new(),
            type_info_offsets: Vec::new(),
            arrays_pending: Vec::new(),
            strings_pending: Vec::new(),
            pointers_pending: Vec::new(),
            boxes_pending: Vec::new(),
            arrays: Vec::new(),
            boxed: Vec::new(),
            arrays_offset: 0,
            boxed_offset: 0,
            empty_array_target: 0,
        }
    }

    fn run(mut self) -> Result<Vec<u8>> {
        self.validate()?;
        self.write_instances()?;
        self.write_arrays()?;
        self.write_boxed_values()?;
        let string_offset = checked_u32(self.payload.len(), "EBX string offset")?;
        self.write_strings()?;
        self.resolve_pointers()?;
        let fixup = self.write_fixup(string_offset)?;
        let extra = self.write_extra()?;
        self.write_riff(&fixup, &extra)
    }

    fn validate(&self) -> Result<()> {
        let document = self.document;
        if document.reflection_data.is_empty() {
            bail!("EBX has no donor reflection table");
        }
        if document.fixup_type_signatures.len() > document.fixup_type_guids.len() {
            bail!("EBX fixup type table is invalid");
        }
        let mut internal_seen = false;
        for instance in &document.instances {
            internal_seen |= !instance.exported;
            if internal_seen && instance.exported {
                bail!("exported EBX instances must precede internal instances");
            }
            if instance.descriptor < 0 || instance.descriptor as usize >= document.types.len() {
                bail!("EBX instance has no valid type descriptor");
            }
            if instance.fixup_type as usize >= document.fixup_type_guids.len() {
                bail!("EBX instance has no valid fixup type");
            }
        }
        Ok(())
    }

    fn grow(&mut self, end: usize) {
        if end > self.payload.len() {
            self.payload.resize(end, 0);
        }
    }

    fn put(&mut self, offset: usize, bytes: &[u8]) {
        self.grow(offset + bytes.len());
        self.payload[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    fn put_u16(&mut self, offset: usize, value: u16) {
        self.put(offset, &value.to_le_bytes());
    }

    fn put_u32(&mut self, offset: usize, value: u32) {
        self.put(offset, &value.to_le_bytes());
    }

    fn put_u64(&mut self, offset: usize, value: u64) {
        self.put(offset, &value.to_le_bytes());
    }

    fn value_for(object: Option<&'a Object>, descriptor: usize) -> Option<&'a Value> {
        object?.fields.iter().find(|field| field.descriptor == descriptor).map(|field| &field.value)
    }

    fn write_instances(&mut self) -> Result<()> {
        let document = self.document;
        let mut cursor = 0;
        for instance in &document.instances {
            let ty = &document.types[instance.descriptor as usize];
            let size = ty.size as usize;
            let start = align_up(cursor + if instance.exported { 16 } else { 0 }, (ty.alignment as usize).max(1));
            if instance.exported {
                self.put(start - 16, &instance.instance_guid.0);
            }
            self.grow(start + size);
            if !instance.raw_image.is_empty() {
                let image = &instance.raw_image[..instance.raw_image.len().min(size)];
                self.put(start, image);
            }
            self.put_u16(start, instance.fixup_type);
            self.write_type(instance.descriptor as usize, start, instance.object.as_ref())?;
            self.instance_offsets.push(checked_u32(start, "EBX instance offset")?);
            cursor = start + size;
        }
        Ok(())
    }

    fn write_type(&mut self, type_index: usize, start: usize, object: Option<&'a Object>) -> Result<()> {
        let document = self.document;
        let ty = &document.types[type_index];
        self.grow(start + ty.size as usize);
        for ordinal in 0..ty.field_count as usize {
            let descriptor = ty.field_index as usize + ordinal;
            let Some(field) = document.fields.get(descriptor) else { bail!("EBX field range is invalid") };
            if field.kind() == kind::INHERITED {
                if field.class_ref as usize >= document.types.len() {
                    bail!("EBX base type is invalid");
                }
                self.write_type(field.class_ref as usize, start, object)?;
                continue;
            }
            let position = start + field.data_offset as usize;
            let value = Self::value_for(object, descriptor);
            if field.is_array() {
                self.grow(position + 8);
                let slot = checked_u32(position, "EBX array slot")?;
                self.pointer_offsets.push(slot);
                let values: &'a [Value] = match value {
                    Some(Value::Array(values)) => values,
                    _ => &[],
                };
                self.arrays_pending.push(PendingArray { slot, values, descriptor, emitted: false });
            } else if field.kind() == kind::STRUCTURE {
                let Some(nested_type) = document.types.get(field.class_ref as usize) else {
                    bail!("EBX struct type is invalid");
                };
                let nested_start = align_up(position, (nested_type.alignment as usize).max(1));
                let nested = match value {
                    Some(Value::Object(object)) => Some(&**object),
                    _ => None,
                };
                self.write_type(field.class_ref as usize, nested_start, nested)?;
            } else {
                self.write_scalar(field, position, value.unwrap_or(NO_VALUE))?;
            }
        }
        Ok(())
    }

    fn write_scalar(&mut self, field: &FieldDescriptor, position: usize, value: &'a Value) -> Result<()> {
        match field.kind() {
            kind::BOOLEAN => {
                let truth = match value {
                    Value::Float(v) => {
                        if !v.is_finite() {
                            bail!("EBX floating-point field must be finite");
                        }
                        *v != 0.0
                    }
                    other => integer(other, i128::MIN, i128::MAX)? != 0,
                };
                self.put(position, &[truth as u8]);
            }
            kind::INT8 => self.put(position, &(integer(value, i8::MIN as i128, i8::MAX as i128)? as i8).to_le_bytes()),
            kind::UINT8 => self.put(position, &[integer(value, 0, u8::MAX as i128)? as u8]),
            kind::INT16 => {
                self.put(position, &(integer(value, i16::MIN as i128, i16::MAX as i128)? as i16).to_le_bytes())
            }
            kind::UINT16 => self.put_u16(position, integer(value, 0, u16::MAX as i128)? as u16),
            kind::INT32 | kind::ENUMERATION => {
                self.put(position, &(integer(value, i32::MIN as i128, i32::MAX as i128)? as i32).to_le_bytes())
            }
            kind::UINT32 => self.put_u32(position, integer(value, 0, u32::MAX as i128)? as u32),
            kind::INT64 => {
                self.put(position, &(integer(value, i64::MIN as i128, i64::MAX as i128)? as i64).to_le_bytes())
            }
            kind::UINT64 => self.put_u64(position, integer(value, 0, u64::MAX as i128)? as u64),
            kind::FLOAT32 => self.put(position, &(float(value, true)? as f32).to_le_bytes()),
            kind::FLOAT64 => self.put(position, &float(value, false)?.to_le_bytes()),
            kind::GUID => {
                let guid = if let Value::Guid(guid) = value { *guid } else { Guid::default() };
                self.put(position, &guid.0);
            }
            kind::SHA1 => {
                let sha = if let Value::Sha1(sha) = value { *sha } else { Sha1::default() };
                self.put(position, &sha.0);
            }
            kind::STRING => {
                let mut field = [0u8; 32];
                if let Value::Bytes(text) = value {
                    if text.len() > 31 {
                        bail!("EBX fixed string exceeds 31 bytes");
                    }
                    field[..text.len()].copy_from_slice(text);
                }
                self.put(position, &field);
            }
            kind::CSTRING | kind::FILE_REF => {
                self.put_u64(position, 0);
                let slot = checked_u32(position, "EBX string slot")?;
                self.pointer_offsets.push(slot);
                let text: &'a [u8] = match value {
                    Value::Bytes(text) => text,
                    _ => &[],
                };
                if text.len() > 1024 * 1024 {
                    bail!("EBX string exceeds the authoring limit");
                }
                self.strings_pending.push((slot, text));
            }
            kind::RESOURCE_REF => {
                let id = if let Value::Resource(id) = value { *id } else { 0 };
                self.put_u64(position, id);
                self.resource_offsets.push(checked_u32(position, "EBX resource slot")?);
            }
            kind::POINTER => {
                self.put_u64(position, 0);
                let pointer = if let Value::Pointer(pointer) = value { *pointer } else { PointerReference::default() };
                self.pointers_pending.push((checked_u32(position, "EBX pointer slot")?, pointer));
            }
            kind::TYPE_REF | kind::DELEGATE => {
                let encoded = if let Value::TypeRef(reference) = value { reference.encoded } else { 0 };
                self.put_u32(position, encoded);
                self.put_u32(position + 4, 0);
                if encoded != 0 && field.kind() == kind::TYPE_REF {
                    self.type_info_offsets.push(checked_u32(position, "EBX type-info slot")?);
                }
            }
            kind::BOXED_VALUE_REF => match value {
                Value::Boxed(reference) if reference.encoded_type != 0 => {
                    self.put_u32(position, reference.encoded_type);
                    self.put_u32(position + 4, 0);
                    self.grow(position + 16);
                    self.type_info_offsets.push(checked_u32(position, "EBX boxed type slot")?);
                    self.pointer_offsets.push(checked_u32(position + 8, "EBX boxed pointer slot")?);
                    self.boxes_pending.push((checked_u32(position, "EBX boxed slot")?, *reference));
                }
                _ => self.put(position, &[0; 16]),
            },
            other => self.grow(position + scalar_size(other)),
        }
        Ok(())
    }

    fn element_stride(&self, field: &FieldDescriptor) -> Result<usize> {
        if field.kind() == kind::STRUCTURE {
            let Some(ty) = self.document.types.get(field.class_ref as usize) else {
                bail!("EBX array struct type is invalid");
            };
            return Ok((ty.size as usize).max(1));
        }
        Ok(scalar_size(field.kind()))
    }

    fn element_alignment(&self, field: &FieldDescriptor) -> Result<usize> {
        if field.kind() == kind::STRUCTURE {
            return Ok((self.document.types[field.class_ref as usize].alignment as usize).max(1));
        }
        Ok(self.element_stride(field)?.clamp(1, 8))
    }

    fn fixup_slot_for_descriptor(&self, descriptor: u16) -> u16 {
        let document = self.document;
        let Some(ty) = document.types.get(descriptor as usize) else { return 0xFFFF };
        document
            .fixup_type_guids
            .iter()
            .zip(&document.fixup_type_signatures)
            .position(|(guid, signature)| *guid == ty.guid && *signature == ty.signature)
            .map_or(0xFFFF, |index| index as u16)
    }

    fn array_hash(&self, field: &FieldDescriptor) -> u32 {
        let raw_type = field.encoded_type << 1;
        let class_ref =
            if field.kind() == kind::STRUCTURE { self.fixup_slot_for_descriptor(field.class_ref) } else { 0xFFFF };
        let arrays = &self.document.arrays;
        if let Some(matching) =
            arrays.iter().find(|array| array.encoded_type == raw_type && array.class_ref == class_ref)
        {
            return matching.hash;
        }
        arrays.first().map_or(field.name_hash, |array| array.hash)
    }

    fn write_arrays(&mut self) -> Result<()> {
        self.arrays_offset = checked_u32(align_up(self.payload.len(), 16), "EBX array section")?;
        self.grow(self.arrays_offset as usize + 32);
        self.empty_array_target = self.arrays_offset + 16;
        let mut index = 0;
        while index < self.arrays_pending.len() {
            if !self.arrays_pending[index].emitted {
                self.emit_array(index)?;
            }
            index += 1;
        }
        Ok(())
    }

    fn emit_array(&mut self, index: usize) -> Result<()> {
        let document = self.document;
        self.arrays_pending[index].emitted = true;
        let PendingArray { slot, values, descriptor, .. } = self.arrays_pending[index];
        let field = &document.fields[descriptor];
        if values.is_empty() {
            self.put_u32(slot as usize, self.empty_array_target.wrapping_sub(slot));
            return Ok(());
        }
        let data = align_up(self.payload.len() + 4, self.element_alignment(field)?);
        let stride = self.element_stride(field)?;
        let count = checked_u32(values.len(), "EBX array count")?;
        self.put_u32(data - 4, count);
        self.grow(data + stride * values.len());
        let data_u32 = checked_u32(data, "EBX array data")?;
        self.put_u32(slot as usize, data_u32.wrapping_sub(slot));
        let first_child = self.arrays_pending.len();
        for (element, value) in values.iter().enumerate() {
            let element_start = data + element * stride;
            if field.kind() == kind::STRUCTURE {
                let object = match value {
                    Value::Object(object) => Some(&**object),
                    _ => None,
                };
                self.write_type(field.class_ref as usize, element_start, object)?;
            } else {
                self.write_scalar(field, element_start, value)?;
            }
        }
        let class_ref =
            if field.kind() == kind::STRUCTURE { self.fixup_slot_for_descriptor(field.class_ref) } else { 0xFFFF };
        self.arrays.push(ArrayRecord {
            offset: data_u32,
            count,
            hash: self.array_hash(field),
            encoded_type: field.encoded_type << 1,
            class_ref,
        });
        for child in first_child..self.arrays_pending.len() {
            if !self.arrays_pending[child].emitted {
                self.emit_array(child)?;
            }
        }
        Ok(())
    }

    fn write_boxed_values(&mut self) -> Result<()> {
        let document = self.document;
        self.boxed_offset = checked_u32(align_up(self.payload.len(), 16), "EBX boxed section")?;
        self.grow(self.boxed_offset as usize);
        let mut moved: HashMap<u32, u32> = HashMap::new();
        let mut records: Vec<&BoxedValueRecord> = document.boxed_values.iter().collect();
        records.sort_by_key(|record| record.offset);
        for source in records {
            if source.offset < document.boxed_values_offset {
                bail!("EBX boxed record precedes its section");
            }
            let offset = self.boxed_offset as usize + (source.offset - document.boxed_values_offset) as usize;
            self.put(offset, &source.raw_bytes);
            let relocate = |from: &[u32], to: &mut Vec<u32>| -> Result<()> {
                for &old in from {
                    if old < source.offset || (old - source.offset) as usize >= source.raw_bytes.len() {
                        continue;
                    }
                    to.push(checked_u32(offset + (old - source.offset) as usize, "EBX boxed relocation")?);
                }
                Ok(())
            };
            relocate(&document.pointer_offsets, &mut self.pointer_offsets)?;
            relocate(&document.resource_ref_offsets, &mut self.resource_offsets)?;
            relocate(&document.import_offsets, &mut self.import_offsets)?;
            relocate(&document.type_info_offsets, &mut self.type_info_offsets)?;
            let offset = checked_u32(offset, "EBX boxed value")?;
            moved.insert(source.offset, offset);
            let mut authored = source.clone();
            authored.offset = offset;
            self.boxed.push(authored);
        }
        for (slot, reference) in self.boxes_pending.clone() {
            self.put_u32(slot as usize, reference.encoded_type);
            self.put_u32(slot as usize + 4, 0);
            let relative = match moved.get(&(reference.data_offset as u32)) {
                Some(&found) => found as i64 - (slot as i64 + 8),
                None => reference.relative_offset,
            };
            self.put(slot as usize + 8, &relative.to_le_bytes());
        }
        Ok(())
    }

    fn write_strings(&mut self) -> Result<()> {
        let mut offsets: HashMap<&'a [u8], u32> = HashMap::new();
        for (slot, text) in std::mem::take(&mut self.strings_pending) {
            let offset = match offsets.get(text) {
                Some(&offset) => offset,
                None => {
                    let offset = checked_u32(self.payload.len(), "EBX string table")?;
                    offsets.insert(text, offset);
                    let end = self.payload.len();
                    self.put(end, text);
                    self.grow(self.payload.len() + 1);
                    offset
                }
            };
            self.put_u32(slot as usize, offset.wrapping_sub(slot));
            self.put_u32(slot as usize + 4, 0);
        }
        Ok(())
    }

    fn resolve_pointers(&mut self) -> Result<()> {
        for (slot, pointer) in std::mem::take(&mut self.pointers_pending) {
            match pointer.kind {
                PointerKind::Null => {}
                PointerKind::Internal => {
                    let Some(&target) =
                        usize::try_from(pointer.index).ok().and_then(|index| self.instance_offsets.get(index))
                    else {
                        bail!("EBX internal pointer index is invalid");
                    };
                    self.put_u32(slot as usize, target.wrapping_sub(slot));
                    self.pointer_offsets.push(slot);
                }
                PointerKind::External => {
                    if pointer.index < 0 || pointer.index as usize >= self.document.imports.len() {
                        bail!("EBX import pointer index is invalid");
                    }
                    self.put_u32(slot as usize, ((pointer.index as u32) << 1) | 1);
                    self.import_offsets.push(slot);
                }
            }
            self.put_u32(slot as usize + 4, 0);
        }
        Ok(())
    }

    fn write_table(writer: &mut Writer, mut values: Vec<u32>) -> Result<()> {
        values.sort_unstable();
        values.dedup();
        writer.u32(checked_u32(values.len(), "EBX fixup table count")?, LE);
        for value in values {
            writer.u32(value, LE);
        }
        Ok(())
    }

    fn write_fixup(&self, string_offset: u32) -> Result<Vec<u8>> {
        let document = self.document;
        let mut writer = Writer::new();
        writer.guid(&document.file_guid, LE);
        writer.u32(checked_u32(document.fixup_type_guids.len(), "EBX type GUID count")?, LE);
        for guid in &document.fixup_type_guids {
            writer.guid(guid, LE);
        }
        writer.u32(checked_u32(document.fixup_type_signatures.len(), "EBX signature count")?, LE);
        for &signature in &document.fixup_type_signatures {
            writer.u32(signature, LE);
        }
        let exported = document.instances.iter().filter(|instance| instance.exported).count();
        writer.u32(checked_u32(exported, "EBX exported instance count")?, LE);
        Self::write_table(&mut writer, self.instance_offsets.clone())?;
        Self::write_table(&mut writer, self.pointer_offsets.clone())?;
        Self::write_table(&mut writer, self.resource_offsets.clone())?;
        writer.u32(checked_u32(document.imports.len(), "EBX import count")?, LE);
        for import in &document.imports {
            writer.guid(&import.file_guid, LE);
            writer.guid(&import.class_guid, LE);
        }
        Self::write_table(&mut writer, self.import_offsets.clone())?;
        Self::write_table(&mut writer, self.type_info_offsets.clone())?;
        writer.u32(self.arrays_offset, LE);
        writer.u32(self.boxed_offset, LE);
        writer.u32(string_offset, LE);

        let mut old_slots = document.boxed_value_slots.clone();
        old_slots.sort_unstable();
        let mut new_slots: Vec<u32> = self.boxes_pending.iter().map(|(slot, _)| *slot).collect();
        new_slots.sort_unstable();
        let trailing = document
            .boxed_reference_offsets
            .iter()
            .filter_map(|old| {
                let index = old_slots.partition_point(|slot| slot < old);
                (old_slots.get(index) == Some(old)).then(|| new_slots.get(index).copied()).flatten()
            })
            .collect();
        Self::write_table(&mut writer, trailing)?;
        Ok(writer.into_inner())
    }

    fn write_extra(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        writer.u32(checked_u32(self.arrays.len(), "EBX array descriptor count")?, LE);
        writer.u32(checked_u32(self.boxed.len(), "EBX boxed descriptor count")?, LE);
        let mut arrays = self.arrays.clone();
        arrays.sort_by_key(|array| array.offset);
        for array in arrays {
            writer.u32(array.offset, LE);
            writer.u32(array.count, LE);
            writer.u32(array.hash, LE);
            writer.u16(array.encoded_type, LE);
            writer.u16(array.class_ref, LE);
        }
        let mut boxed: Vec<&BoxedValueRecord> = self.boxed.iter().collect();
        boxed.sort_by_key(|record| record.offset);
        for record in boxed {
            writer.u32(record.offset, LE);
            writer.u32(record.count, LE);
            writer.u32(record.hash, LE);
            writer.u16(record.encoded_type, LE);
            writer.u16(record.class_ref, LE);
        }
        Ok(writer.into_inner())
    }

    fn write_riff(&self, fixup: &[u8], extra: &[u8]) -> Result<Vec<u8>> {
        let document = self.document;
        let mut writer = Writer::new();
        writer.bytes(b"RIFF");
        writer.u32(0, LE);
        // EBXS requires native-layout conversion when loaded by the game.
        writer.bytes(if document.serialized_layout { b"EBXS" } else { b"EBX\0" });
        let mut chunk = |name: &[u8; 4], bytes: &[u8]| -> Result<()> {
            writer.bytes(name);
            writer.u32(checked_u32(bytes.len(), "EBX RIFF chunk size")?, LE);
            writer.bytes(bytes);
            if bytes.len() & 1 != 0 {
                writer.u8(0);
            }
            Ok(())
        };
        // 12 bytes of padding put the payload on a 16-byte boundary (RIFF header 12 + chunk 8).
        let mut data = vec![0u8; 12];
        data.extend_from_slice(&self.payload);
        chunk(b"EBXD", &data)?;
        chunk(b"EFIX", fixup)?;
        chunk(b"EBXX", extra)?;
        chunk(if document.hashed_reflection_names { b"RFL2" } else { b"REFL" }, &document.reflection_data)?;
        let mut result = writer.into_inner();
        let size = checked_u32(result.len() - 8, "EBX RIFF size")?;
        result[4..8].copy_from_slice(&size.to_le_bytes());
        Ok(result)
    }
}

pub fn write_document(document: &Document) -> Result<Vec<u8>> {
    Builder::new(document).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT_GUID: Guid = Guid([0xA1; 16]);
    const ITEM_GUID: Guid = Guid([0xB2; 16]);

    /// A REFL chunk for two types: `Root` (a class) and `Item` (a struct).
    fn reflection() -> Vec<u8> {
        let mut strings = Vec::new();
        let mut name = |text: &str| {
            let offset = strings.len() as u32;
            strings.extend_from_slice(text.as_bytes());
            strings.push(0);
            offset
        };
        let names: Vec<u32> = ["Root", "Item", "Name", "Count", "Items", "Ref", "Id", "Size"].map(&mut name).to_vec();
        let mut w = Writer::new();
        w.i32(2, LE);
        w.guid(&ROOT_GUID, LE);
        w.u32(0x1111, LE);
        w.guid(&ITEM_GUID, LE);
        w.u32(0x2222, LE);
        w.i32(2, LE);
        // Root: fields 0..4, size 48, align 8. Item: fields 4..6, size 24, align 8.
        for (name, first, count, size) in [(names[0], 0, 4, 48), (names[1], 4, 2, 24)] {
            w.u32(name, LE);
            w.i32(first, LE);
            w.u16(count, LE);
            w.u16(0x0101 << 1, LE);
            w.u16(size, LE);
            w.u16(8, LE);
        }
        let field = |w: &mut Writer, name: u32, offset: u32, ty: u8, category: u8, class_ref: u16| {
            w.u32(name, LE);
            w.u32(offset, LE);
            w.u16((((ty as u16) << 4) | category as u16) << 1, LE);
            w.u16(class_ref, LE);
        };
        w.i32(6, LE);
        field(&mut w, names[2], 8, kind::CSTRING, 3, 0);
        field(&mut w, names[3], 16, kind::UINT32, 3, 0);
        field(&mut w, names[4], 24, kind::STRUCTURE, kind::CATEGORY_ARRAY, 1);
        field(&mut w, names[5], 32, kind::POINTER, 1, 0);
        field(&mut w, names[6], 0, kind::GUID, 3, 0);
        field(&mut w, names[7], 16, kind::UINT32, 3, 0);
        w.i32(0, LE); // groups
        w.i32(0, LE); // mappings
        w.bytes(&strings);
        w.into_inner()
    }

    fn document() -> Document {
        let mut document = Document {
            reflection_data: reflection(),
            fixup_type_guids: vec![ROOT_GUID, ITEM_GUID],
            fixup_type_signatures: vec![0x1111, 0x2222],
            imports: vec![ImportReference { file_guid: Guid([7; 16]), class_guid: Guid([8; 16]) }],
            ..Default::default()
        };
        // Parse the reflection table the same way the reader does.
        let reflection = document.reflection_data.clone();
        let chunk = Chunk { id: REFL, begin: 0, end: reflection.len() };
        read_reflection(&reflection, &mut Reader::new(&reflection), chunk, false, &mut document).unwrap();
        let item = |id: u8, size: u64| {
            Value::Object(Box::new(Object {
                descriptor: 1,
                fields: vec![
                    FieldValue { descriptor: 4, name: "Id".into(), value: Value::Guid(Guid([id; 16])) },
                    FieldValue { descriptor: 5, name: "Size".into(), value: Value::UInt(size) },
                ],
            }))
        };
        let root = |name: &str, items: Vec<Value>, pointer: PointerReference| Object {
            descriptor: 0,
            fields: vec![
                FieldValue { descriptor: 0, name: "Name".into(), value: Value::Bytes(name.as_bytes().to_vec()) },
                FieldValue { descriptor: 1, name: "Count".into(), value: Value::UInt(items.len() as u64) },
                FieldValue { descriptor: 2, name: "Items".into(), value: Value::Array(items) },
                FieldValue { descriptor: 3, name: "Ref".into(), value: Value::Pointer(pointer) },
            ],
        };
        document.instances = vec![
            InstanceRecord {
                descriptor: 0,
                exported: true,
                instance_guid: Guid([5; 16]),
                object: Some(root(
                    "Audio/Music/Song",
                    vec![item(1, 100), item(2, 200)],
                    PointerReference { kind: PointerKind::Internal, index: 1 },
                )),
                ..Default::default()
            },
            InstanceRecord {
                descriptor: 0,
                object: Some(root("inner", vec![], PointerReference { kind: PointerKind::External, index: 0 })),
                ..Default::default()
            },
        ];
        document
    }

    #[test]
    fn written_documents_read_back_and_rewrite_identically() {
        let source = document();
        let bytes = write_document(&source).unwrap();
        let mut back = read_document(&bytes).unwrap();
        assert_eq!(back.root_type, "Root");
        assert_eq!(back.instances.len(), 2);
        assert_eq!(back.instances[0].instance_guid, Guid([5; 16]));
        for (read, written) in back.instances.iter().zip(&source.instances) {
            assert_eq!(read.object, written.object);
        }
        assert_eq!(write_document(&back).unwrap(), bytes);

        // Editing a string and growing an array still round-trips.
        let root = back.object_of("Root").unwrap();
        *root.field_mut("Name").unwrap() = Value::Bytes(b"Audio/Music/Assets/ReSkate/X_MG".to_vec());
        let Value::Array(items) = root.field_mut("Items").unwrap() else { panic!("not an array") };
        let extra = items[0].clone();
        items.push(extra);
        let edited = write_document(&back).unwrap();
        let again = read_document(&edited).unwrap();
        let root = again.instances[0].object.as_ref().unwrap();
        assert_eq!(root.field("Name").unwrap(), &Value::Bytes(b"Audio/Music/Assets/ReSkate/X_MG".to_vec()));
        let Value::Array(items) = root.field("Items").unwrap() else { panic!("not an array") };
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn out_of_range_integers_are_refused() {
        let mut source = document();
        *source.object_of("Root").unwrap().field_mut("Count").unwrap() = Value::Int(-1);
        assert!(write_document(&source).is_err());
    }
}
