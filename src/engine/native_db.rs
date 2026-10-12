// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-FileCopyrightText: 2026 the ReSkate contributors
// SPDX-License-Identifier: GPL-3.0-only
// Ported from the ReSkate project (https://github.com/Dingo-Shenanigans/ReSkate); see NOTICE.md.
//! The native DB container (InitFS, layout.toc, superbundle TOCs): a tree of tagged values.

use crate::{Result, bail};

/// Signed header in front of every native DB file. Studio writes it all zero apart from the
/// magic, which the runtime verifier accepts without a signature check.
pub const ENVELOPE_SIZE: usize = 0x22C;
pub const MAGIC: [u8; 4] = [0x00, 0xD1, 0xCE, 0x01];

/// Value types used here: 1 list, 2 object, 6 bool, 7 string, 8 int32, 19 blob.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub kind: u8,
    pub named: bool,
    pub name: String,
    /// The string without its trailing NUL, for kind 7.
    pub text: String,
    pub bytes: Vec<u8>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn field(&self, key: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.named && child.name == key)
    }
}

pub struct Options {
    /// InitFS requires one value per key; layout.toc's meta section repeats keys by design.
    pub unique_fields: bool,
}

pub fn read(input: &[u8], context: &str, options: Options) -> Result<Node> {
    let mut reader = DbReader { input, position: 0, context, options, nodes: 0 };
    reader.node(0)
}

struct DbReader<'a> {
    input: &'a [u8],
    position: usize,
    context: &'a str,
    options: Options,
    nodes: usize,
}

impl DbReader<'_> {
    fn fail<T>(&self, message: &str) -> Result<T> {
        bail!("{}: {message}", self.context)
    }

    fn take(&mut self, size: usize) -> Result<&[u8]> {
        if size > self.input.len() - self.position {
            return self.fail("Truncated DB value");
        }
        let value = &self.input[self.position..self.position + size];
        self.position += size;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn variable(&mut self) -> Result<usize> {
        let mut value = 0usize;
        let mut shift = 0;
        while shift < 63 {
            let byte = self.byte()?;
            value |= ((byte & 127) as usize) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
        self.fail("Invalid DB variable length")
    }

    fn node(&mut self, depth: usize) -> Result<Node> {
        self.nodes += 1;
        if depth > 32 || self.nodes > 32768 {
            return self.fail("DB nesting/entry limit exceeded");
        }
        let tag = self.byte()?;
        let mut node = Node { kind: tag & 31, ..Default::default() };
        if node.kind == 0 {
            return Ok(node);
        }
        node.named = tag & 128 == 0;
        if node.named {
            let mut name = Vec::new();
            loop {
                let ch = self.byte()?;
                if ch == 0 {
                    break;
                }
                if name.len() >= 1024 {
                    return self.fail("DB field name too long");
                }
                name.push(ch);
            }
            node.name = String::from_utf8_lossy(&name).into_owned();
        }
        match node.kind {
            1 | 2 => {
                let size = self.variable()?;
                if size > self.input.len() - self.position {
                    return self.fail("Truncated DB container");
                }
                let end = self.position + size;
                while self.position < end {
                    let child = self.node(depth + 1)?;
                    if child.kind == 0 {
                        break;
                    }
                    if self.options.unique_fields && node.kind == 2 && child.named && node.field(&child.name).is_some()
                    {
                        return self.fail("Duplicate DB field");
                    }
                    node.children.push(child);
                }
                if self.position != end {
                    return self.fail("DB container length mismatch");
                }
            }
            7 | 19 => {
                let size = self.variable()?;
                node.bytes = self.take(size)?.to_vec();
                if node.kind == 7 {
                    if node.bytes.len() > 65536 {
                        return self.fail("DB string too long");
                    }
                    let text = node.bytes.strip_suffix(&[0]).unwrap_or(&node.bytes);
                    node.text = String::from_utf8_lossy(text).into_owned();
                }
            }
            kind => {
                let size = match kind {
                    6 => 1,
                    8 | 11 => 4,
                    9 | 12 => 8,
                    15 => 16,
                    16 => 20,
                    _ => return self.fail("Unsupported DB value type"),
                };
                node.bytes = self.take(size)?.to_vec();
            }
        }
        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_object_with_a_list_of_records() {
        // { "a": "hi", "l": [ { "n": 5 } ] }
        let record = [8u8, b'n', 0, 5, 0, 0, 0, 0];
        let mut list = vec![0x80 | 2, record.len() as u8];
        list.extend_from_slice(&record);
        let mut body = vec![7, b'a', 0, 3, b'h', b'i', 0, 1, b'l', 0, list.len() as u8];
        body.extend_from_slice(&list);
        body.push(0);
        let mut bytes = vec![0x80 | 2, body.len() as u8];
        bytes.extend_from_slice(&body);
        let root = read(&bytes, "test", Options { unique_fields: true }).unwrap();
        assert_eq!(root.field("a").unwrap().text, "hi");
        let entry = &root.field("l").unwrap().children[0];
        assert_eq!(entry.field("n").unwrap().bytes, [5, 0, 0, 0]);
    }
}
