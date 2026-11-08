//! Small directory capability: no output operation resolves an ambient path.

use std::fs::{File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::fs::{Mode, OFlags, RenameFlags, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub(super) const MAX_RECORD_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub(super) struct Directory(pub File);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecordPoint {
    Created,
    Written,
    Synced,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    pub fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    pub fn archive_name(self) -> String {
        format!("saved-{:x}-{:x}", self.device, self.inode)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fingerprint {
    pub identity: Identity,
    pub digest: [u8; 32],
    pub length: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

pub(super) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

impl Directory {
    pub fn root(path: &Path) -> io::Result<Self> {
        Ok(Self(File::from(rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?)))
    }

    pub fn child(&self, name: &str) -> io::Result<Self> {
        Ok(Self(File::from(openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?)))
    }

    pub fn identity(&self) -> io::Result<Identity> {
        Ok(Identity::of(&self.0.metadata()?))
    }

    pub fn make_child(&self, name: &str) -> io::Result<Self> {
        rustix::fs::mkdirat(&self.0, name, Mode::RWXU)?;
        self.sync()?;
        self.child(name)
    }

    pub fn open(&self, name: &str) -> io::Result<File> {
        let file = File::from(openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("expected a regular file"));
        }
        Ok(file)
    }

    pub fn create(&self, name: &str) -> io::Result<File> {
        Ok(File::from(openat(
            &self.0,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?))
    }

    pub fn exists(&self, name: &str) -> io::Result<bool> {
        match rustix::fs::statat(&self.0, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn regular_entry(&self, name: &str) -> io::Result<Option<bool>> {
        match rustix::fs::statat(&self.0, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => Ok(Some(
                rustix::fs::FileType::from_raw_mode(metadata.st_mode)
                    == rustix::fs::FileType::RegularFile,
            )),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    // dev_t is signed on macOS and unsigned on Linux. Treat its bits as an
    // opaque persistent identity; no arithmetic or narrowing uses this value.
    #[allow(clippy::cast_sign_loss, clippy::unnecessary_cast)]
    pub fn entry_identity(&self, name: &str) -> io::Result<Identity> {
        let metadata = rustix::fs::statat(&self.0, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
        Ok(Identity {
            device: metadata.st_dev as u64,
            inode: metadata.st_ino as u64,
        })
    }

    pub fn fingerprint(&self, name: &str) -> io::Result<Option<Fingerprint>> {
        match self.open(name) {
            Ok(mut file) => Ok(Some(fingerprint(&mut file)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn read_record(&self, name: &str) -> io::Result<Vec<u8>> {
        self.read_record_with_limit(name, MAX_RECORD_BYTES)
    }

    pub fn read_record_with_limit(&self, name: &str, max_bytes: u64) -> io::Result<Vec<u8>> {
        let counted = max_bytes
            .checked_add(1)
            .ok_or_else(|| io::Error::other("invalid recovery record byte limit"))?;
        let mut bytes = Vec::new();
        self.open(name)?.take(counted).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max_bytes {
            return Err(io::Error::other("recovery record exceeds its byte limit"));
        }
        Ok(bytes)
    }

    pub fn record(
        &self,
        name: &str,
        bytes: &[u8],
        hook: &mut impl FnMut(RecordPoint) -> io::Result<()>,
    ) -> io::Result<()> {
        self.record_with_limit(name, bytes, MAX_RECORD_BYTES, hook)
    }

    pub fn record_with_limit(
        &self,
        name: &str,
        bytes: &[u8],
        max_bytes: u64,
        hook: &mut impl FnMut(RecordPoint) -> io::Result<()>,
    ) -> io::Result<()> {
        if bytes.len() as u64 > max_bytes {
            return Err(io::Error::other("recovery record exceeds its byte limit"));
        }
        // Interrupted partial records remain private and unreferenced. Never
        // publish partial JSON or truncate a prior attempt during recovery.
        for attempt in 0..64 {
            let pending = format!(".pending-{name}-{attempt}");
            let mut file = match self.create(&pending) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            };
            hook(RecordPoint::Created)?;
            file.write_all(bytes)?;
            hook(RecordPoint::Written)?;
            file.sync_all()?;
            hook(RecordPoint::Synced)?;
            self.rename_to(&pending, self, name)?;
            hook(RecordPoint::Published)?;
            return self.sync();
        }
        Err(io::Error::other(
            "too many interrupted record attempts; retained files require inspection",
        ))
    }

    pub fn rename_to(&self, name: &str, destination: &Self, new: &str) -> io::Result<()> {
        rustix::fs::renameat_with(&self.0, name, &destination.0, new, RenameFlags::NOREPLACE)?;
        Ok(())
    }

    pub fn sync(&self) -> io::Result<()> {
        self.0.sync_all()
    }

    pub fn entry_names(&self, max_entries: usize) -> io::Result<Vec<String>> {
        let directory = rustix::fs::Dir::read_from(&self.0)?;
        let mut names = Vec::new();
        for entry in directory {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| io::Error::other("non-UTF-8 recovery entry"))?;
            if matches!(name, "." | "..") {
                continue;
            }
            if names.len() == max_entries {
                return Err(io::Error::other("recovery directory entry limit exceeded"));
            }
            names.push(name.to_owned());
        }
        names.sort_unstable();
        Ok(names)
    }
}

pub(super) fn fingerprint(file: &mut File) -> io::Result<Fingerprint> {
    let before = file.metadata()?;
    if !before.is_file() || before.len() > MAX_FILE_BYTES {
        return Err(io::Error::other(
            "expected a regular file of at most 32 MiB",
        ));
    }
    let mut hasher = Sha256::new();
    let mut count = 0;
    let mut buffer = [0; 16 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        count += size as u64;
        if count > MAX_FILE_BYTES {
            return Err(io::Error::other(
                "file grew beyond 32 MiB during observation",
            ));
        }
        hasher.update(&buffer[..size]);
    }
    let after = file.metadata()?;
    // Renaming legitimately changes ctime. Check it only across this read,
    // never as a persisted baseline or after our own namespace operations.
    if Identity::of(&before) != Identity::of(&after)
        || before.len() != count
        || after.len() != count
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(io::Error::other("file changed during observation"));
    }
    Ok(Fingerprint {
        identity: Identity::of(&after),
        digest: hasher.finalize().into(),
        length: count,
        mode: after.mode() & 0o7777,
        uid: after.uid(),
        gid: after.gid(),
    })
}

pub(super) fn open_parent<'a>(
    root: &Directory,
    path: &'a str,
) -> io::Result<(Directory, Vec<Identity>, &'a str)> {
    let mut components: Vec<_> = path.split('/').collect();
    let name = components
        .pop()
        .ok_or_else(|| io::Error::other("empty path"))?;
    let mut directory = Directory(root.0.try_clone()?);
    let mut identities = Vec::with_capacity(components.len());
    for component in components {
        directory = directory.child(component)?;
        identities.push(directory.identity()?);
    }
    Ok((directory, identities, name))
}
