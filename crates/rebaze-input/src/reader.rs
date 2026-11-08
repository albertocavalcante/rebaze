use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, Metadata, OpenOptions};

use crate::{LimitKind, ReadLimits, Usage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkPolicy {
    /// Relative links may resolve only within the original opened root.
    WithinRoot,
    /// Every descendant component, including the final file, rejects symlinks.
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// An owned name and observed kind; retaining this value holds no directory FD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    name: OsString,
    kind: EntryKind,
}

impl DirectoryEntry {
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    pub const fn kind(&self) -> EntryKind {
        self.kind
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("relative input path is too long: limit 4096 bytes, observed {bytes}")]
    PathTooLong { bytes: usize },
    #[error("invalid relative input path: {0:?}")]
    InvalidPath(PathBuf),
    #[error("input {path:?} exceeds {kind} budget: limit {limit}, observed {observed}")]
    Limit {
        path: PathBuf,
        kind: LimitKind,
        limit: u64,
        observed: u64,
    },
    #[error("input must be a regular file: {0:?}")]
    NotRegular(PathBuf),
    #[error("input changed during reading: {0:?}")]
    Changed(PathBuf),
    #[error("input is not UTF-8: {path:?}: {source}")]
    Utf8 {
        path: PathBuf,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("input IO failed at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl ReadError {
    pub fn into_io_error(self) -> io::Error {
        let kind = match &self {
            Self::Io { source, .. } => source.kind(),
            Self::Limit { .. } => io::ErrorKind::FileTooLarge,
            Self::InvalidPath(_) | Self::PathTooLong { .. } => io::ErrorKind::InvalidInput,
            Self::NotRegular(_) | Self::Changed(_) | Self::Utf8 { .. } => {
                io::ErrorKind::InvalidData
            }
        };
        io::Error::new(kind, self)
    }
}

/// One operation's IO authority and shared monotonic budget. Deliberately not
/// Clone: descending into another module must not reset aggregate usage.
pub struct RootReader {
    root: Dir,
    limits: ReadLimits,
    policy: SymlinkPolicy,
    usage: Usage,
}

impl RootReader {
    /// Open an explicitly selected ambient root once. Descendant reads never
    /// reopen its display pathname. Absolute symlink targets are unsupported.
    pub fn open(root: &Path, limits: ReadLimits, policy: SymlinkPolicy) -> Result<Self, ReadError> {
        let directory = Dir::open_ambient_dir(root, cap_std::ambient_authority())
            .map_err(|source| io_error(root, source))?;
        Ok(Self {
            root: directory,
            limits,
            policy,
            usage: Usage::default(),
        })
    }

    pub const fn usage(&self) -> Usage {
        self.usage
    }

    /// Inspect the final entry without following it. Its parent must exist.
    pub fn kind(&mut self, path: &Path) -> Result<Option<EntryKind>, ReadError> {
        self.begin(path, true)?;
        if path.components().all(|part| part == Component::CurDir) {
            return self
                .root
                .dir_metadata()
                .map(|metadata| Some(entry_kind(&metadata)))
                .map_err(|source| io_error(path, source));
        }
        // Resolve the parent separately: a missing/broken ancestor is an error,
        // rather than evidence that an optional final file is absent.
        let (parent, name) = match self.policy {
            SymlinkPolicy::Reject => self.reject_parent(path)?,
            SymlinkPolicy::WithinRoot => {
                let name = path
                    .file_name()
                    .ok_or_else(|| ReadError::InvalidPath(path.into()))?;
                let parent = self
                    .root
                    .open_dir(root_path(path.parent().unwrap_or_else(|| Path::new(""))))
                    .map_err(|source| io_error(path, source))?;
                (parent, name)
            }
        };
        let metadata = parent
            .symlink_metadata(name)
            .map_err(|source| io_error(path, source));
        match metadata {
            Ok(metadata) => Ok(Some(entry_kind(&metadata))),
            Err(ReadError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Read an optional final file.
    ///
    /// A missing ancestor, broken link, special file or exhausted budget is an
    /// error; only an absent final entry is None.
    pub fn read_optional_text(&mut self, path: &Path) -> Result<Option<String>, ReadError> {
        match self.kind(path)? {
            None => Ok(None),
            Some(_) => self.read_text(path).map(Some),
        }
    }

    /// Return a canonical root-relative directory path without ambient lookups.
    /// This path is not a stable inode identity or a snapshot.
    pub fn canonical_dir(&mut self, path: &Path) -> Result<PathBuf, ReadError> {
        self.begin(path, true)?;
        match self.policy {
            SymlinkPolicy::WithinRoot => {
                let canonical = self
                    .root
                    .canonicalize(root_path(path))
                    .map_err(|source| io_error(path, source))?;
                self.root
                    .open_dir(root_path(&canonical))
                    .map_err(|source| io_error(path, source))?;
                Ok(canonical)
            }
            SymlinkPolicy::Reject => {
                let _directory = self.reject_directory(path)?;
                Ok(path.to_owned())
            }
        }
    }

    pub fn read_text(&mut self, path: &Path) -> Result<String, ReadError> {
        self.read_with(path, &mut |_| Ok(()))
    }

    /// List an existing directory in name order, observing links without following them.
    ///
    /// Every yielded entry is charged before decoding, filtering or sorting. The
    /// first over-budget proof entry remains charged and prevents further scans.
    pub fn directory_entries(&mut self, path: &Path) -> Result<Vec<DirectoryEntry>, ReadError> {
        self.begin(path, true)?;
        self.entry_budget(path)?;
        let directory = match self.policy {
            SymlinkPolicy::WithinRoot => self
                .root
                .open_dir(root_path(path))
                .map_err(|source| io_error(path, source))?,
            SymlinkPolicy::Reject => self.reject_directory(path)?,
        };
        let iterator = directory
            .entries()
            .map_err(|source| io_error(path, source))?;
        self.collect_directory(path, iterator)
    }

    fn collect_directory(
        &mut self,
        path: &Path,
        mut iterator: impl Iterator<Item = io::Result<cap_std::fs::DirEntry>>,
    ) -> Result<Vec<DirectoryEntry>, ReadError> {
        let mut entries = Vec::new();
        loop {
            charge(
                &mut self.usage.operations,
                self.limits.operations,
                path,
                LimitKind::Operations,
            )?;
            let Some(entry) = iterator.next() else {
                break;
            };
            self.usage.entries = self.usage.entries.checked_add(1).ok_or_else(|| {
                io_error(path, io::Error::other("directory entry count overflow"))
            })?;
            self.entry_budget(path)?;
            let entry = entry.map_err(|source| io_error(path, source))?;
            let name = entry.file_name();
            let child = child_path(path, &name)?;
            self.begin(&child, false)?;
            let kind = file_type_kind(
                entry
                    .file_type()
                    .map_err(|source| io_error(&child, source))?,
            );
            entries.push(DirectoryEntry { name, kind });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    fn entry_budget(&self, path: &Path) -> Result<(), ReadError> {
        if self.usage.entries > self.limits.entries {
            return Err(limit(
                path,
                LimitKind::DirectoryEntries,
                self.limits.entries,
                self.usage.entries as u64,
            ));
        }
        Ok(())
    }

    fn read_with(
        &mut self,
        path: &Path,
        hook: &mut impl FnMut(Point) -> io::Result<()>,
    ) -> Result<String, ReadError> {
        self.begin(path, false)?;
        charge(
            &mut self.usage.files,
            self.limits.files,
            path,
            LimitKind::FileAttempts,
        )?;
        if self.usage.bytes > self.limits.total_bytes {
            return Err(limit(
                path,
                LimitKind::AggregateBytes,
                self.limits.total_bytes,
                self.usage.bytes as u64,
            ));
        }
        let preflight = self.file_metadata(path)?;
        if !preflight.is_file() {
            return Err(ReadError::NotRegular(path.into()));
        }
        self.size_check(path, preflight.len())?;
        hook(Point::AfterMetadata).map_err(|source| io_error(path, source))?;
        let mut options = OpenOptions::new();
        options.read(true).nonblock(true).follow(match self.policy {
            SymlinkPolicy::WithinRoot => FollowSymlinks::Yes,
            SymlinkPolicy::Reject => FollowSymlinks::No,
        });
        let mut file = match self.policy {
            SymlinkPolicy::WithinRoot => self.root.open_with(path, &options),
            SymlinkPolicy::Reject => {
                let (parent, name) = self.reject_parent(path)?;
                parent.open_with(name, &options)
            }
        }
        .map_err(|source| io_error(path, source))?;
        let before = file.metadata().map_err(|source| io_error(path, source))?;
        if !before.is_file() {
            return Err(ReadError::NotRegular(path.into()));
        }
        self.size_check(path, before.len())?;
        let modified = before.modified().map_err(|source| io_error(path, source))?;
        hook(Point::AfterOpen).map_err(|source| io_error(path, source))?;
        let bytes = self.counted(path, &mut file)?;
        let after = file.metadata().map_err(|source| io_error(path, source))?;
        if before.len() != bytes.len() as u64
            || after.len() != before.len()
            || after.modified().map_err(|source| io_error(path, source))? != modified
        {
            return Err(ReadError::Changed(path.into()));
        }
        String::from_utf8(bytes).map_err(|error| ReadError::Utf8 {
            path: path.into(),
            source: error.utf8_error(),
        })
    }

    fn counted(&mut self, path: &Path, reader: &mut impl Read) -> Result<Vec<u8>, ReadError> {
        let mut bytes = Vec::new();
        let mut file_bytes = 0_usize;
        let mut buffer = [0; 8192];
        loop {
            if self.usage.bytes > self.limits.total_bytes {
                return Err(limit(
                    path,
                    LimitKind::AggregateBytes,
                    self.limits.total_bytes,
                    self.usage.bytes as u64,
                ));
            }
            charge(
                &mut self.usage.operations,
                self.limits.operations,
                path,
                LimitKind::Operations,
            )?;
            let remaining = self
                .limits
                .file_bytes
                .saturating_sub(file_bytes)
                .min(self.limits.total_bytes.saturating_sub(self.usage.bytes));
            let length = remaining
                .checked_add(1)
                .ok_or_else(|| io_error(path, io::Error::other("input count overflow")))?
                .min(buffer.len());
            let count = match reader.read(&mut buffer[..length]) {
                Ok(0) => return Ok(bytes),
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(source) => return Err(io_error(path, source)),
            };
            self.usage.bytes =
                self.usage.bytes.checked_add(count).ok_or_else(|| {
                    io_error(path, io::Error::other("aggregate byte count overflow"))
                })?;
            file_bytes = file_bytes
                .checked_add(count)
                .ok_or_else(|| io_error(path, io::Error::other("file byte count overflow")))?;
            if file_bytes > self.limits.file_bytes {
                return Err(limit(
                    path,
                    LimitKind::FileBytes,
                    self.limits.file_bytes,
                    file_bytes as u64,
                ));
            }
            if self.usage.bytes > self.limits.total_bytes {
                return Err(limit(
                    path,
                    LimitKind::AggregateBytes,
                    self.limits.total_bytes,
                    self.usage.bytes as u64,
                ));
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
    }

    fn size_check(&self, path: &Path, observed: u64) -> Result<(), ReadError> {
        if observed > self.limits.file_bytes as u64 {
            return Err(limit(
                path,
                LimitKind::FileBytes,
                self.limits.file_bytes,
                observed,
            ));
        }
        Ok(())
    }

    fn begin(&mut self, path: &Path, directory: bool) -> Result<(), ReadError> {
        charge(
            &mut self.usage.operations,
            self.limits.operations,
            path,
            LimitKind::Operations,
        )?;
        let mut depth = 0_usize;
        let path_bytes = path.as_os_str().as_encoded_bytes().len();
        if path_bytes > 4096 {
            return Err(ReadError::PathTooLong { bytes: path_bytes });
        }
        for component in path.components() {
            match component {
                Component::Normal(_) => depth += 1,
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(ReadError::InvalidPath(path.into()));
                }
            }
        }
        if depth == 0 && !directory {
            return Err(ReadError::InvalidPath(path.into()));
        }
        if depth > self.limits.depth {
            return Err(limit(
                path,
                LimitKind::PathDepth,
                self.limits.depth,
                depth as u64,
            ));
        }
        Ok(())
    }

    fn file_metadata(&self, path: &Path) -> Result<Metadata, ReadError> {
        let result = match self.policy {
            SymlinkPolicy::WithinRoot => self.root.metadata(path),
            SymlinkPolicy::Reject => {
                let (parent, name) = self.reject_parent(path)?;
                parent.symlink_metadata(name)
            }
        };
        result.map_err(|source| io_error(path, source))
    }

    fn reject_directory(&self, path: &Path) -> Result<Dir, ReadError> {
        let mut directory = self
            .root
            .try_clone()
            .map_err(|source| io_error(path, source))?;
        for component in path.components() {
            if let Component::Normal(name) = component {
                directory = directory
                    .open_dir_nofollow(name)
                    .map_err(|source| io_error(path, source))?;
            }
        }
        Ok(directory)
    }

    fn reject_parent<'a>(&self, path: &'a Path) -> Result<(Dir, &'a OsStr), ReadError> {
        let name = path
            .file_name()
            .ok_or_else(|| ReadError::InvalidPath(path.into()))?;
        let parent = self.reject_directory(path.parent().unwrap_or_else(|| Path::new("")))?;
        Ok((parent, name))
    }
}

fn child_path(parent: &Path, name: &OsStr) -> Result<PathBuf, ReadError> {
    let parent_bytes = parent.as_os_str().as_encoded_bytes();
    let separator = usize::from(
        !parent_bytes.is_empty()
            && !parent_bytes
                .last()
                .is_some_and(|byte| *byte == b'/' || cfg!(windows) && *byte == b'\\'),
    );
    let bytes = parent_bytes
        .len()
        .checked_add(separator)
        .and_then(|value| value.checked_add(name.as_encoded_bytes().len()))
        .ok_or_else(|| io_error(parent, io::Error::other("directory path length overflow")))?;
    if bytes > 4096 {
        return Err(ReadError::PathTooLong { bytes });
    }
    Ok(parent.join(name))
}

fn root_path(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    }
}
fn entry_kind(metadata: &Metadata) -> EntryKind {
    file_type_kind(metadata.file_type())
}
fn file_type_kind(kind: cap_std::fs::FileType) -> EntryKind {
    if kind.is_file() {
        EntryKind::File
    } else if kind.is_dir() {
        EntryKind::Directory
    } else if kind.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::Other
    }
}
fn io_error(path: &Path, source: io::Error) -> ReadError {
    ReadError::Io {
        path: error_path(path),
        source,
    }
}
fn limit(path: &Path, kind: LimitKind, bound: usize, observed: u64) -> ReadError {
    ReadError::Limit {
        path: error_path(path),
        kind,
        limit: bound as u64,
        observed,
    }
}
fn error_path(path: &Path) -> PathBuf {
    if path.as_os_str().as_encoded_bytes().len() > 4096 {
        PathBuf::from("<oversized input path>")
    } else {
        path.into()
    }
}
fn charge(value: &mut usize, bound: usize, path: &Path, kind: LimitKind) -> Result<(), ReadError> {
    let next = value
        .checked_add(1)
        .ok_or_else(|| io_error(path, io::Error::other("input work count overflow")))?;
    if next > bound {
        return Err(limit(path, kind, bound, next as u64));
    }
    *value = next;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    AfterMetadata,
    AfterOpen,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod directory_tests;
