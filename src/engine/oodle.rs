// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! Oodle (Kraken / Selkie / Leviathan) CAS blocks.
//!
//! Decoding prefers the game's own `oo2core_*_win64.dll` on Windows, exactly as the C++ tool did,
//! and otherwise uses `oozextract`, an open-source decoder, so the game's data can also be read
//! on macOS and Linux (for a game installed under Wine/CrossOver). Encoding needs the game's DLL;
//! where it is not available the caller stores the block raw, which the game reads as well.

use std::path::Path;

use crate::{Result, bail};

/// Decodes one Oodle block into exactly `size` bytes.
pub fn decompress(input: &[u8], size: usize, game_root: &Path) -> Result<Vec<u8>> {
    #[cfg(windows)]
    if let Some(library) = native::library(game_root) {
        return native::decompress(library, input, size);
    }
    let _ = game_root;
    let mut output = vec![0u8; size];
    match oozextract::Extractor::new().read_from_slice(input, &mut output) {
        Ok(written) if written == size => Ok(output),
        Ok(_) => bail!("Oodle CAS block failed to decode (short output)"),
        Err(error) => bail!("Oodle CAS block failed to decode: {error}"),
    }
}

/// Encodes one block with the given Oodle compressor (Kraken = 8, Selkie = 11, Leviathan = 13).
/// `None` when no encoder is available on this system.
pub fn compress(input: &[u8], compressor: i32, level: i32, game_root: &Path) -> Result<Option<Vec<u8>>> {
    #[cfg(windows)]
    if let Some(library) = native::library(game_root) {
        return native::compress(library, input, compressor, level).map(Some);
    }
    let _ = (input, compressor, level, game_root);
    Ok(None)
}

/// Whether blocks will be Oodle-compressed (true) or stored raw (false) on this system.
pub fn can_compress(game_root: &Path) -> bool {
    #[cfg(windows)]
    {
        native::library(game_root).is_some()
    }
    #[cfg(not(windows))]
    {
        let _ = game_root;
        false
    }
}

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use libloading::os::windows::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library};

    use crate::{Result, bail};

    type Decompress = unsafe extern "C" fn(
        *const c_void,
        i64,
        *mut c_void,
        i64,
        i32,
        i32,
        i32,
        *mut c_void,
        i64,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        i64,
        i32,
    ) -> i64;
    type BufferSize = unsafe extern "C" fn(i32, i64) -> i64;
    type Compress = unsafe extern "C" fn(
        i32,
        *const c_void,
        i64,
        *mut c_void,
        i32,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        i64,
    ) -> i64;

    /// The game's module, resolved once and never unloaded.
    static LIBRARY: OnceLock<Option<Library>> = OnceLock::new();

    fn find(game_root: &Path) -> Option<PathBuf> {
        let mut matches: Vec<PathBuf> = std::fs::read_dir(game_root)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
                name.starts_with("oo2core_") && name.ends_with("_win64.dll") && path.is_file()
            })
            .collect();
        matches.sort();
        matches.pop()
    }

    pub fn library(game_root: &Path) -> Option<&'static Library> {
        LIBRARY
            .get_or_init(|| {
                let path = find(game_root)?;
                // SAFETY: the game's own Oodle module; loading runs only its DllMain.
                unsafe {
                    Library::load_with_flags(&path, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32)
                }
                .ok()
            })
            .as_ref()
    }

    pub fn decompress(library: &Library, input: &[u8], size: usize) -> Result<Vec<u8>> {
        // SAFETY: the export has this signature in every oo2core 9 build.
        let decode = unsafe { library.get::<Decompress>(b"OodleLZ_Decompress\0") }
            .map_err(|_| crate::Error::msg("Oodle export is missing: OodleLZ_Decompress"))?;
        let mut output = vec![0u8; size];
        // SAFETY: both buffers are valid for the lengths passed.
        let written = unsafe {
            decode(
                input.as_ptr().cast(),
                input.len() as i64,
                output.as_mut_ptr().cast(),
                size as i64,
                1,
                0,
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                3,
            )
        };
        if written != size as i64 {
            bail!("Oodle CAS block failed to decode");
        }
        Ok(output)
    }

    pub fn compress(library: &Library, input: &[u8], compressor: i32, level: i32) -> Result<Vec<u8>> {
        // SAFETY: the exports have these signatures in every oo2core 9 build.
        let (bound, encode) = unsafe {
            (
                library.get::<BufferSize>(b"OodleLZ_GetCompressedBufferSizeNeeded\0"),
                library.get::<Compress>(b"OodleLZ_Compress\0"),
            )
        };
        let (Ok(bound), Ok(encode)) = (bound, encode) else { bail!("Oodle compression exports are missing") };
        // SAFETY: plain integer arguments.
        let capacity = unsafe { bound(compressor, input.len() as i64) };
        if capacity <= 0 {
            bail!("Oodle could not size its compression buffer");
        }
        let mut output = vec![0u8; capacity as usize];
        // SAFETY: both buffers are valid for the lengths passed; the options are defaults.
        let length = unsafe {
            encode(
                compressor,
                input.as_ptr().cast(),
                input.len() as i64,
                output.as_mut_ptr().cast(),
                level,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if length <= 0 || length > capacity {
            bail!("Oodle CAS compression failed");
        }
        output.truncate(length as usize);
        Ok(output)
    }
}
