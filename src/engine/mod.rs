// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! The Frostbite package formats the packer reads and writes: TOCs, bundle manifests, CAS
//! archives and EBX partitions, plus read access to the installed game's data.

pub mod binary;
pub mod bundle;
pub mod cas;
pub mod ebx;
pub mod game;
pub mod native_db;
pub mod oodle;
pub mod toc;
