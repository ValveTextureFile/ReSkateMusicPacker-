// SPDX-FileCopyrightText: 2026 DeckardDetribine and the ReSkateMusicPacker contributors
// SPDX-License-Identifier: GPL-3.0-only
//! One error type for the whole library. Every message is meant to be shown to the user as is.

use std::io;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A long job stopped because its cancel flag was set; nothing was left behind.
    #[error("Cancelled")]
    Cancelled,
    #[error("{0}")]
    Message(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Error::Message(text.into())
    }

    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Error::Io { context: context.into(), source }
    }
}

/// `return Err(Error::Message(format!(...)))`.
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::Error::Message(format!($($arg)*))) };
}

/// Adds the path to an I/O error so the message says which file failed.
pub trait IoContext<T> {
    fn at(self, action: &str, path: &Path) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn at(self, action: &str, path: &Path) -> Result<T> {
        self.map_err(|source| Error::io(format!("{action} {}", path.display()), source))
    }
}
