#[cfg(not(mobile))]
pub mod file;
#[cfg(not(mobile))]
pub mod grep;
#[cfg(mobile)]
mod mobile;
#[cfg(not(mobile))]
pub mod mutate;
#[cfg(not(mobile))]
pub mod search;
#[cfg(not(mobile))]
pub mod tree;

#[cfg(mobile)]
pub use mobile::{file, grep, mutate, search, tree};

#[cfg(not(mobile))]
use std::path::Path;

/// Frontend-facing path: forward-slash on every platform.
#[cfg(not(mobile))]
pub fn to_canon(p: impl AsRef<Path>) -> String {
    let s = p.as_ref().to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        s.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        s
    }
}
