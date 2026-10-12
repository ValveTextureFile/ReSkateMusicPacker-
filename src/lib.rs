// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! ReSkate Music Packer: a folder of songs in, an add-only music mod for skate. out.
//!
//! [`engine`] holds the Frostbite formats; [`music`] is the packer built on them.

pub mod assets;
pub mod engine;
pub mod error;
pub mod music;
pub mod settings;

pub use error::{Error, Result};
