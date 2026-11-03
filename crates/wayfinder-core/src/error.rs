//! One error type for the store, the index, and the graph.

use std::fmt::{self, Display, Formatter};

/// Everything a wayfinder operation can fail with.
#[derive(Debug)]
pub enum Error {
    /// The page file could not be read or written.
    Io(String),
    /// The page's frontmatter is missing or malformed.
    Frontmatter(String),
    /// The pool directory does not exist yet; `init` creates it.
    NoPool(String),
    /// A page id is unknown to the pool.
    UnknownPage(String),
    /// A write carried a revision that no longer matches the file.
    StaleRevision {
        /// Page the write targeted.
        id: String,
        /// Revision the writer expected.
        expected: String,
        /// Revision the file carries now.
        current: String,
    },
    /// The derived index could not be built or read.
    Index(String),
    /// An argument the caller supplied cannot be honoured.
    Invalid(String),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "{message}"),
            Self::Frontmatter(message) => write!(f, "frontmatter: {message}"),
            Self::NoPool(path) => write!(
                f,
                "no wayfinder pool here ({path}); run `wayfinder init` first"
            ),
            Self::UnknownPage(id) => write!(f, "no memory page with id \"{id}\""),
            Self::StaleRevision {
                id,
                expected,
                current,
            } => write!(
                f,
                "page \"{id}\" changed since it was read (expected rev {expected}, found {current}); read it again before saving"
            ),
            Self::Index(message) => write!(f, "index: {message}"),
            Self::Invalid(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Index(error.to_string())
    }
}

/// Result of any wayfinder operation.
pub type Result<T> = std::result::Result<T, Error>;
