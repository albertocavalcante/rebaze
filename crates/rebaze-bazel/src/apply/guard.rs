//! Root binding, private journal setup and cooperative locking.

use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::fs::{Directory, Identity, fingerprint, open_parent};
use super::{ApplyError, Manifest, PRIVATE_DIRECTORY, Point, RecordKind, SCHEMA};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootBinding {
    schema: u32,
    root: PathBuf,
    identity: Identity,
    directory: Identity,
}

pub(super) struct Guard {
    pub(super) root: PathBuf,
    root_directory: Directory,
    pub(super) directory: Directory,
    _lock: File,
}

impl Guard {
    #[cfg(test)]
    pub(super) fn open(root: &Path) -> Result<Self, ApplyError> {
        Self::open_with(root, None, &mut |_| Ok(()))
    }

    pub(super) fn open_with(
        root: &Path,
        expected: Option<&Manifest>,
        hook: &mut impl FnMut(Point) -> io::Result<()>,
    ) -> Result<Self, ApplyError> {
        let result = (|| -> io::Result<_> {
            let root = root.canonicalize()?;
            if root.to_str().is_none() {
                return Ok(Err(ApplyError::Invalid("canonical root must be UTF-8")));
            }
            let root_directory = Directory::root(&root)?;
            if let Some(expected) = expected
                && (root != expected.root || root_directory.identity()? != expected.root_identity)
            {
                return Ok(Err(ApplyError::Invalid(
                    "canonical root identity changed before application",
                )));
            }
            let directory = match root_directory.make_child(PRIVATE_DIRECTORY) {
                Ok(directory) => directory,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    root_directory.child(PRIVATE_DIRECTORY)?
                }
                Err(error) => return Err(error),
            };
            let metadata = directory.0.metadata()?;
            if metadata.mode() & 0o777 != 0o700
                || metadata.uid() != rustix::process::geteuid().as_raw()
            {
                return Err(io::Error::other(
                    "recovery directory must have mode 0700 and belong to the effective user",
                ));
            }
            let lock = match directory.create("lock") {
                Ok(lock) => {
                    lock.sync_all()?;
                    directory.sync()?;
                    lock
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    directory.open("lock")?
                }
                Err(error) => return Err(error),
            };
            if metadata.uid() != lock.metadata()?.uid() || lock.metadata()?.nlink() != 1 {
                return Err(io::Error::other(
                    "recovery lock must belong to the directory owner and have no other hard links",
                ));
            }
            match lock.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return Ok(Err(ApplyError::Locked)),
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
            // A lock must be anchored to the same entry other cooperative writers
            // open; do not accept a stale descriptor after namespace replacement.
            if fingerprint(&mut directory.open("lock")?)?.identity
                != Identity::of(&lock.metadata()?)
            {
                return Err(io::Error::other("lock entry changed during acquisition"));
            }
            match directory.read_record(".gitignore") {
                Ok(bytes) if bytes == b"*\n" => {}
                Ok(_) => {
                    return Err(io::Error::other(
                        "recovery .gitignore has unexpected content",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    directory.record(".gitignore", b"*\n", &mut |point| {
                        hook(Point::Record(RecordKind::Ignore, point))
                    })?;
                }
                Err(error) => return Err(error),
            }
            let binding = RootBinding {
                schema: SCHEMA,
                root: root.clone(),
                identity: root_directory.identity()?,
                directory: directory.identity()?,
            };
            match directory.read_record("root.json") {
                Ok(bytes) => {
                    let observed: RootBinding =
                        serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    if observed.schema != SCHEMA
                        || observed.root != binding.root
                        || observed.identity != binding.identity
                        || observed.directory != binding.directory
                    {
                        return Err(io::Error::other(
                            "recovery directory belongs to another root",
                        ));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    directory.record(
                        "root.json",
                        &serde_json::to_vec(&binding).map_err(io::Error::other)?,
                        &mut |point| hook(Point::Record(RecordKind::Root, point)),
                    )?;
                }
                Err(error) => return Err(error),
            }
            Ok(Ok(Self {
                root,
                root_directory,
                directory,
                _lock: lock,
            }))
        })();
        result.map_err(|source| ApplyError::Io {
            retained: root.join(PRIVATE_DIRECTORY),
            source,
        })?
    }

    pub(super) fn io<T>(&self, result: io::Result<T>) -> Result<T, ApplyError> {
        result.map_err(|source| ApplyError::Io {
            retained: self.root.join(PRIVATE_DIRECTORY),
            source,
        })
    }

    pub(super) fn conflict(&self, reason: &'static str) -> ApplyError {
        ApplyError::Conflict {
            reason,
            retained: self.root.join(PRIVATE_DIRECTORY),
        }
    }

    pub(super) fn invalid(&self, reason: &'static str) -> ApplyError {
        ApplyError::InvalidRecord {
            reason,
            retained: self.root.join(PRIVATE_DIRECTORY).join("active"),
        }
    }

    pub(super) fn ensure_root(&self, plan: &Manifest) -> Result<(), ApplyError> {
        if self.root != plan.root
            || self.io(self.root_directory.identity())? != plan.root_identity
            || self.io(Directory::root(&self.root).and_then(|root| root.identity()))?
                != plan.root_identity
        {
            return Err(self.conflict("canonical root identity changed"));
        }
        Ok(())
    }

    pub(super) fn parent<'a>(
        &self,
        plan: &'a Manifest,
    ) -> Result<(Directory, &'a str), ApplyError> {
        self.ensure_root(plan)?;
        let (parent, identities, name) = self.io(open_parent(&self.root_directory, &plan.path))?;
        if identities != plan.parents {
            return Err(self.conflict("output parent directory identity changed"));
        }
        Ok((parent, name))
    }
}
