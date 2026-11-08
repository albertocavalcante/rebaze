//! Explicit, bounded IO under one opened project-directory authority.
//!
//! Parsing belongs to callers. This library never executes build tools, writes
//! project files, searches configuration or prints output. An opened root stays
//! authoritative across nominal pathname changes; it is not an input snapshot.
//!
//! Defaults: 1 MiB per file, 64 MiB total consumed bytes, 4,096 file attempts,
//! 65,536 logical operations and 64 relative path components. Oversize proof
//! bytes and invalid UTF-8 remain charged. Callers can choose validated limits.
//! Symlink policy is explicit; absolute link targets are unsupported. Capabilities
//! do not establish hard-link/mount provenance or deadlines for regular-file IO.
//! Ordinary Unix FIFO race tests require the POSIX `mkfifo` utility.
//!
//! ```no_run
//! use std::path::Path;
//! use rebaze_input::{ReadLimits, RootReader, SymlinkPolicy};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut input = RootReader::open(
//!     Path::new("project"), ReadLimits::default(), SymlinkPolicy::Reject,
//! )?;
//! let source = input.read_text(Path::new("settings.gradle.kts"))?;
//! assert!(input.usage().bytes() >= source.len());
//! # Ok(())
//! # }
//! ```

mod limits;
mod reader;

pub use limits::{LimitKind, ReadLimits, ReadLimitsError, Usage};
pub use reader::{EntryKind, ReadError, RootReader, SymlinkPolicy};
