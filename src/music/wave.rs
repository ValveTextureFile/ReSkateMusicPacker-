// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The wave's sound-bank resource.
//!
//! Every NewWaveAsset has a bundle resource of this type under the same name: an "SBle"
//! sound-bank blob. Loading it registers the wave's instance GUID with the audio system
//! (Skate.exe sub_141485D30 -> sub_14148BE50); a wave without one is looked up there anyway when
//! it is instantiated (sub_1414841B0) and the game crashes on the not-found sentinel.

use crate::engine::binary::Guid;
use crate::{Result, bail};

pub const WAVE_RESOURCE_TYPE: u32 = 0xb2c4_65f6;

/// What differs between two songs' blobs, by value. The blob is the template's with each of these
/// replaced; a value found other than the expected number of times stops the build rather than
/// leave a half-patched blob.
#[derive(Clone, Debug, Default)]
pub struct WaveFacts {
    pub instance: Guid,
    pub prefetch_id: Guid,
    pub stream_id: Guid,
    pub prefetch_size: u32,
    pub stream_size: u32,
    /// The SoundBank duration, matching the H header's playable sample total.
    pub samples: u64,
    pub name: String,
}

/// djb2 with xor.
pub fn djb2(text: &[u8]) -> u32 {
    text.iter().fold(5381u32, |hash, &byte| hash.wrapping_mul(33) ^ byte as u32)
}

/// The NameHash of an asset: djb2-xor of its name's last component, as the shipped assets carry it.
pub fn name_hash(name: &str) -> u32 {
    djb2(name.rsplit('/').next().unwrap_or(name).as_bytes())
}

fn replace_all(blob: &mut [u8], from: &[u8], to: &[u8], expected: usize, what: &str) -> Result<()> {
    debug_assert_eq!(from.len(), to.len());
    let mut found = 0;
    let mut at = 0;
    while at + from.len() <= blob.len() {
        if &blob[at..at + from.len()] == from {
            blob[at..at + from.len()].copy_from_slice(to);
            found += 1;
            at += from.len();
        } else {
            at += 1;
        }
    }
    if found != expected {
        bail!("Template wave resource: {what} found {found} time(s), expected {expected}");
    }
    Ok(())
}

pub fn wave_resource(template: &[u8], from: &WaveFacts, to: &WaveFacts) -> Result<Vec<u8>> {
    let mut blob = template.to_vec();
    if blob.len() < 220 {
        bail!("Template wave resource is too short");
    }
    let id = |guid: &Guid| djb2(guid.to_string().as_bytes()).to_le_bytes();
    // An id derived from the wave: djb2-xor of its instance GUID as text (confirmed on the template).
    replace_all(&mut blob, &id(&from.instance), &id(&to.instance), 7, "the wave id")?;
    // A second per-wave id whose source is not known; it only has to be unique, so it is derived
    // from the new wave's name. Unknown semantics: find its source if a song misbehaves.
    let unknown: [u8; 4] = blob[216..220].try_into().expect("4 bytes");
    replace_all(&mut blob, &unknown, &djb2(to.name.as_bytes()).to_le_bytes(), 2, "the second wave id")?;
    let seconds = |samples: u64| ((samples as f64 / 48000.0) as f32).to_le_bytes();
    replace_all(&mut blob, &seconds(from.samples), &seconds(to.samples), 1, "the duration")?;
    replace_all(&mut blob, &from.prefetch_size.to_le_bytes(), &to.prefetch_size.to_le_bytes(), 1, "the prefetch size")?;
    let length = |facts: &WaveFacts| facts.stream_size.wrapping_sub(facts.prefetch_size).to_le_bytes();
    replace_all(&mut blob, &length(from), &length(to), 2, "the stream length")?;
    replace_all(&mut blob, &from.prefetch_id.0, &to.prefetch_id.0, 1, "the prefetch chunk id")?;
    replace_all(&mut blob, &from.stream_id.0, &to.stream_id.0, 1, "the stream chunk id")?;
    replace_all(&mut blob, &from.instance.0, &to.instance.0, 1, "the wave GUID")?;
    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_hash_uses_the_last_component() {
        assert_eq!(name_hash("Audio/Music/Assets/ReSkate/A_MG"), djb2(b"A_MG"));
        assert_eq!(name_hash("plain"), djb2(b"plain"));
        assert_eq!(djb2(b""), 5381);
    }

    #[test]
    fn replacements_must_match_their_expected_counts() {
        let mut blob = vec![1, 2, 1, 2, 3];
        replace_all(&mut blob, &[1, 2], &[7, 8], 2, "x").unwrap();
        assert_eq!(blob, [7, 8, 7, 8, 3]);
        assert!(replace_all(&mut blob, &[3], &[4], 2, "x").is_err());
    }
}
