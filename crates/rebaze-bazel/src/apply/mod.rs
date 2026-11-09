//! Opt-in guarded single-file and batch application on Linux and macOS.
//!
//! Core still uses the legacy per-file bundle writer. Callers choose the limited
//! POSIX metadata contract. Original entries are retained even after success;
//! recovery never overwrites a recreated destination. See the application guide
//! for filesystem, concurrency and metadata assumptions.

mod batch;
mod fs;
mod guard;

pub use batch::{
    BatchEditPlan, BatchOutcome, BatchReceipt, ConflictReason, OutputDisposition, OutputReport,
    recover_bundle,
};

use std::fs::Permissions;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use self::fs::{Directory, Fingerprint, Identity, MAX_FILE_BYTES, digest, open_parent};
use self::guard::Guard;
use crate::OutputPath;

const PRIVATE_DIRECTORY: &str = ".rebaze-apply";
const SCHEMA: u32 = 1;

/// Explicitly limits preservation to mode, owning user and group.
///
/// Extended ACLs, xattrs, timestamps, hard-link relationships and filesystem flags are outside
/// this opt-in API's contract. Existing set-id/sticky files are rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataPolicy {
    PosixModeAndOwner,
}

/// Captured filesystem evidence, separate from pure generation intent.
/// All fields are private: callers cannot substitute a fabricated baseline.
pub struct FileEditPlan {
    manifest: Manifest,
    content: String,
}

impl std::fmt::Debug for FileEditPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileEditPlan")
            .field("manifest", &self.manifest)
            .field("content_bytes", &self.content.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    root: PathBuf,
    root_identity: Identity,
    path: String,
    parents: Vec<Identity>,
    expected: Option<Fingerprint>,
    attributes: Attributes,
    intended_digest: [u8; 32],
    intended_length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attributes {
    mode: u32,
    uid: u32,
    gid: u32,
}

impl Attributes {
    const fn of(value: &Fingerprint) -> Self {
        Self {
            mode: value.mode,
            uid: value.uid,
            gid: value.gid,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    plan: Manifest,
    plan_identity: [u8; 32],
    transaction: Identity,
    staged: Fingerprint,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreRecord {
    plan_identity: [u8; 32],
    entry: Identity,
    changed: bool,
}

/// A successful terminal state; archived originals remain available at `retained`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "inspect outcome; a terminal recovery state does not imply intended content was applied"]
pub struct ApplyReceipt {
    pub outcome: ApplyOutcome,
    pub retained: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Restored,
    /// A file changed after the initial baseline check and was put back intact.
    RestoredConflict,
    /// No output was moved before staging stopped. The baseline is unverified.
    Abandoned,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("output {path}: {source}")]
    Output {
        path: OutputPath,
        #[source]
        source: Box<Self>,
    },
    #[error("invalid application plan or recovery record: {0}")]
    Invalid(&'static str),
    #[error("invalid recovery record: {reason}; recovery data retained at {retained}")]
    InvalidRecord {
        reason: &'static str,
        retained: PathBuf,
    },
    #[error("another cooperative application holds the project lock")]
    Locked,
    #[error("application conflict: {reason}; recovery data retained at {retained}")]
    Conflict {
        reason: &'static str,
        retained: PathBuf,
    },
    #[error("application IO failed; inspect retained location {retained}: {source}")]
    Io {
        retained: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl FileEditPlan {
    /// Reads an existing root and existing parent directories. No file is written.
    ///
    /// Paths are capped at 4096 bytes / 64 components; file content at 32 MiB.
    ///
    /// ```
    /// use rebaze_bazel::{OutputPath, apply::{FileEditPlan, MetadataPolicy, recover}};
    /// let root = tempfile::tempdir()?;
    /// let path = OutputPath::new("BUILD.bazel".to_owned())?;
    /// let plan = FileEditPlan::capture(root.path(), &path, "# generated\n".to_owned(),
    ///     MetadataPolicy::PosixModeAndOwner)?;
    /// let receipt = plan.apply()?;
    /// assert!(receipt.retained.join("manifest.json").is_file());
    /// assert!(recover(root.path())?.is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn capture(
        root: &Path,
        path: &OutputPath,
        content: String,
        metadata: MetadataPolicy,
    ) -> Result<Self, ApplyError> {
        validate_path(path.as_str())?;
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err(ApplyError::Invalid("intended file exceeds 32 MiB"));
        }
        let result = (|| {
            let root = root.canonicalize()?;
            if root.to_str().is_none() {
                return Err(io::Error::other("canonical root must be UTF-8"));
            }
            let directory = Directory::root(&root)?;
            let root_identity = directory.identity()?;
            let (parent, parents, name) = open_parent(&directory, path.as_str())?;
            let expected = parent.fingerprint(name)?;
            let parent_metadata = parent.0.metadata()?;
            let attributes = match metadata {
                MetadataPolicy::PosixModeAndOwner => expected.as_ref().map_or_else(
                    || Attributes {
                        mode: 0o600,
                        uid: rustix::process::geteuid().as_raw(),
                        gid: if parent_metadata.mode() & 0o2000 != 0 {
                            parent_metadata.gid()
                        } else {
                            rustix::process::getegid().as_raw()
                        },
                    },
                    Attributes::of,
                ),
            };
            if expected
                .as_ref()
                .is_some_and(|value| value.mode & 0o7000 != 0)
            {
                return Err(io::Error::other(
                    "set-id/sticky output files are unsupported",
                ));
            }
            Ok(Self {
                manifest: Manifest {
                    schema: SCHEMA,
                    root,
                    root_identity,
                    path: path.as_str().to_owned(),
                    parents,
                    expected,
                    attributes,
                    intended_digest: digest(content.as_bytes()),
                    intended_length: content.len() as u64,
                },
                content,
            })
        })();
        result.map_err(|source| ApplyError::Io {
            retained: root.join(PRIVATE_DIRECTORY),
            source,
        })
    }

    pub fn path(&self) -> &str {
        &self.manifest.path
    }

    /// Apply or conservatively restore a conflict. Inspect the returned outcome:
    /// `Ok` does not by itself establish that intended content was installed.
    pub fn apply(self) -> Result<ApplyReceipt, ApplyError> {
        self.apply_with(&mut |_| Ok(()))
    }

    fn apply_with(
        self,
        hook: &mut impl FnMut(Point) -> io::Result<()>,
    ) -> Result<ApplyReceipt, ApplyError> {
        let guard = Guard::open_with(&self.manifest.root, Some(&self.manifest), hook)?;
        guard.ensure_root(&self.manifest)?;
        if guard.io(guard.directory.exists("active"))? {
            return Err(guard.conflict("pending transaction requires recovery"));
        }
        let (parent, name) = guard.parent(&self.manifest)?;
        let name = name.to_owned();
        if guard.io(parent.fingerprint(&name))? != self.manifest.expected {
            return Err(guard.conflict("destination no longer matches the captured baseline"));
        }
        guard.io(hook(Point::BaselineChecked))?;
        let active = guard.io(guard.directory.make_child("active"))?;
        let staged = stage_file(
            &guard,
            &active,
            &self.manifest,
            self.content.as_bytes(),
            hook,
        )?;
        let plan_identity = manifest_digest(&self.manifest)?;
        let journal = Journal {
            plan: self.manifest,
            plan_identity,
            transaction: guard.io(active.identity())?,
            staged,
        };
        let record = serde_json::to_vec(&journal).map_err(|_| guard.invalid("journal encoding"))?;
        if record.len() as u64 > fs::MAX_RECORD_BYTES {
            return Err(guard.invalid("journal exceeds 64 KiB"));
        }
        guard.io(active.record("manifest.json", &record, &mut |point| {
            hook(Point::Record(RecordKind::Intent, point))
        }))?;
        guard.io(hook(Point::IntentDurable))?;
        guard.parent(&journal.plan)?;
        if journal.plan.expected.is_some() {
            guard.io(parent.rename_to(&name, &active, "before"))?;
            guard.io(parent.sync())?;
            guard.io(active.sync())?;
            guard.io(hook(Point::Moved))?;
            if active.fingerprint("before").ok().flatten() != journal.plan.expected {
                return restore(&guard, &active, &journal, true, hook);
            }
        }
        guard.io(hook(Point::BeforeInstall))?;
        guard.parent(&journal.plan)?;
        guard.io(active.rename_to("after", &parent, &name))?;
        guard.io(hook(Point::InstallSync))?;
        guard.io(parent.sync())?;
        guard.io(active.sync())?;
        guard.io(hook(Point::Installed))?;
        finish(&guard, &active, &journal, ApplyOutcome::Applied, hook)
    }
}

fn stage_file(
    guard: &Guard,
    active: &Directory,
    manifest: &Manifest,
    content: &[u8],
    hook: &mut impl FnMut(Point) -> io::Result<()>,
) -> Result<Fingerprint, ApplyError> {
    let mut staged = guard.io(active.create("after"))?;
    guard.io(staged.write_all(content))?;
    {
        let attributes = manifest.attributes;
        let metadata = guard.io(staged.metadata())?;
        if metadata.uid() != attributes.uid {
            return Err(guard.conflict("staging cannot preserve the destination owner"));
        }
        if metadata.gid() != attributes.gid {
            guard.io(rustix::fs::fchown(
                &staged,
                None,
                Some(rustix::fs::Gid::from_raw(attributes.gid)),
            )
            .map_err(Into::into))?;
        }
        guard.io(staged.set_permissions(Permissions::from_mode(attributes.mode)))?;
    }
    guard.io(hook(Point::StageSync))?;
    guard.io(staged.sync_all())?;
    drop(staged);
    let staged = guard
        .io(active.fingerprint("after"))?
        .ok_or_else(|| guard.conflict("staged file disappeared"))?;
    if !matches_intent(manifest, &staged) {
        return Err(guard.conflict("staged bytes or metadata differ from the captured intent"));
    }
    Ok(staged)
}

/// Reconcile the one active transaction using actual file identities and bytes.
/// No active transaction returns `None`; completed archives are never removed.
pub fn recover(root: &Path) -> Result<Option<ApplyReceipt>, ApplyError> {
    recover_with(root, &mut |_| Ok(()))
}

fn recover_with(
    root: &Path,
    hook: &mut impl FnMut(Point) -> io::Result<()>,
) -> Result<Option<ApplyReceipt>, ApplyError> {
    let guard = Guard::open_with(root, None, hook)?;
    if !guard.io(guard.directory.exists("active"))? {
        return Ok(None);
    }
    let active = guard.io(guard.directory.child("active"))?;
    for name in guard.io(active.entry_names(256))? {
        if !matches!(
            name.as_str(),
            "after" | "before" | "manifest.json" | "restore.json"
        ) && !pending_record(&name, "manifest.json")
            && !pending_record(&name, "restore.json")
        {
            return Err(guard.invalid(
                "unrecognized single-file layout; batch journals require recover_bundle",
            ));
        }
    }
    if !guard.io(active.exists("manifest.json"))? {
        if guard.io(active.exists("before"))? {
            return Err(guard.conflict("backup exists without durable intent"));
        }
        // The protocol cannot move any destination before this record exists.
        // An interrupted staging attempt therefore needs only archival.
        return archive(&guard, &active, ApplyOutcome::Abandoned, hook).map(Some);
    }
    let journal: Journal = serde_json::from_slice(&guard.io(active.read_record("manifest.json"))?)
        .map_err(|_| guard.invalid("malformed or unsupported journal"))?;
    validate_journal(&guard, &active, &journal)?;
    let (parent, name) = guard.parent(&journal.plan)?;
    if !guard.io(active.exists("before"))? && guard.io(active.exists("restore.json"))? {
        let record: RestoreRecord =
            serde_json::from_slice(&guard.io(active.read_record("restore.json"))?)
                .map_err(|_| guard.invalid("malformed restore record"))?;
        if record.plan_identity != journal.plan_identity
            || guard.io(parent.entry_identity(name))? != record.entry
        {
            return Err(guard.conflict("restored entry changed before recovery completed"));
        }
        guard.io(parent.sync())?;
        let outcome = if record.changed {
            ApplyOutcome::RestoredConflict
        } else {
            ApplyOutcome::Restored
        };
        return finish(&guard, &active, &journal, outcome, hook).map(Some);
    }
    let target = parent.fingerprint(name);
    if target.as_ref().ok().and_then(Option::as_ref) == Some(&journal.staged) {
        // A completed rename may precede directory sync or the commit marker.
        guard.io(guard.io(parent.open(name))?.sync_all())?;
        guard.io(parent.sync())?;
        guard.io(active.sync())?;
        return finish(&guard, &active, &journal, ApplyOutcome::Applied, hook).map(Some);
    }
    if guard.io(active.exists("before"))? {
        if guard.io(parent.exists(name))? {
            return Err(guard.conflict("destination was recreated or edited; refusing replacement"));
        }
        let changed = active.fingerprint("before").ok().flatten() != journal.plan.expected;
        return restore(&guard, &active, &journal, changed, hook).map(Some);
    }
    if guard.io(parent.fingerprint(name))? == journal.plan.expected {
        return finish(&guard, &active, &journal, ApplyOutcome::Restored, hook).map(Some);
    }
    Err(guard.conflict("no verified terminal state; refusing to invent missing content"))
}

fn restore(
    guard: &Guard,
    active: &Directory,
    journal: &Journal,
    changed: bool,
    hook: &mut impl FnMut(Point) -> io::Result<()>,
) -> Result<ApplyReceipt, ApplyError> {
    let outcome = restore_entry(guard, active, journal, changed, hook)?;
    finish(guard, active, journal, outcome, hook)
}

fn restore_entry(
    guard: &Guard,
    active: &Directory,
    journal: &Journal,
    changed: bool,
    hook: &mut impl FnMut(Point) -> io::Result<()>,
) -> Result<ApplyOutcome, ApplyError> {
    let (parent, name) = guard.parent(&journal.plan)?;
    if !guard.io(active.exists("restore.json"))? {
        let record = RestoreRecord {
            plan_identity: journal.plan_identity,
            entry: guard.io(active.entry_identity("before"))?,
            changed,
        };
        guard.io(active.record(
            "restore.json",
            &serde_json::to_vec(&record).map_err(|_| guard.invalid("restore encoding"))?,
            &mut |point| hook(Point::Record(RecordKind::Restore, point)),
        ))?;
    }
    guard.io(hook(Point::BeforeRestore))?;
    guard.io(active.rename_to("before", &parent, name))?;
    guard.io(hook(Point::RestoreSync))?;
    guard.io(parent.sync())?;
    guard.io(active.sync())?;
    let outcome = if changed {
        ApplyOutcome::RestoredConflict
    } else {
        ApplyOutcome::Restored
    };
    Ok(outcome)
}

fn finish(
    guard: &Guard,
    active: &Directory,
    journal: &Journal,
    outcome: ApplyOutcome,
    hook: &mut impl FnMut(Point) -> io::Result<()>,
) -> Result<ApplyReceipt, ApplyError> {
    let (parent, name) = guard.parent(&journal.plan)?;
    match outcome {
        ApplyOutcome::Applied
            if guard.io(parent.fingerprint(name))? != Some(journal.staged.clone()) =>
        {
            return Err(guard.conflict("installed file changed before completion"));
        }
        ApplyOutcome::Restored if guard.io(parent.fingerprint(name))? != journal.plan.expected => {
            return Err(guard.conflict("baseline changed before completion"));
        }
        _ => {}
    }
    archive(guard, active, outcome, hook)
}

fn archive(
    guard: &Guard,
    active: &Directory,
    outcome: ApplyOutcome,
    hook: &mut impl FnMut(Point) -> io::Result<()>,
) -> Result<ApplyReceipt, ApplyError> {
    let retained = archive_directory(guard, active, hook)?;
    Ok(ApplyReceipt { outcome, retained })
}

fn archive_directory(
    guard: &Guard,
    active: &Directory,
    hook: &mut impl FnMut(Point) -> io::Result<()>,
) -> Result<PathBuf, ApplyError> {
    guard.io(hook(Point::BeforeArchive))?;
    // Retain originals, staged files and intent even on success. Never unlink
    // recovery content merely because a mutable destination looked correct once.
    let archive = guard.io(active.identity())?.archive_name();
    guard.io(active.sync())?;
    guard.io(guard
        .directory
        .rename_to("active", &guard.directory, &archive))?;
    let retained = guard.root.join(PRIVATE_DIRECTORY).join(archive);
    hook(Point::ArchiveSync)
        .and_then(|()| guard.directory.sync())
        .map_err(|source| ApplyError::Io {
            retained: retained.clone(),
            source,
        })?;
    Ok(retained)
}

fn pending_record(name: &str, record: &str) -> bool {
    let Some(suffix) = name
        .strip_prefix(".pending-")
        .and_then(|part| part.strip_prefix(record))
        .and_then(|part| part.strip_prefix('-'))
    else {
        return false;
    };
    suffix
        .parse::<u8>()
        .is_ok_and(|attempt| attempt < 64 && attempt.to_string() == suffix)
}

fn validate_path(path: &str) -> Result<(), ApplyError> {
    if path.len() > 4096
        || path.split('/').count() > 64
        || OutputPath::new(path.to_owned()).is_err()
    {
        return Err(ApplyError::Invalid(
            "invalid, reserved or overlong output path",
        ));
    }
    Ok(())
}

fn manifest_digest(manifest: &Manifest) -> Result<[u8; 32], ApplyError> {
    serde_json::to_vec(manifest)
        .map(|bytes| digest(&bytes))
        .map_err(|_| ApplyError::Invalid("plan encoding"))
}

fn validate_journal(
    guard: &Guard,
    active: &Directory,
    journal: &Journal,
) -> Result<(), ApplyError> {
    let plan = &journal.plan;
    validate_path(&plan.path).map_err(|_| guard.invalid("invalid output path in journal"))?;
    guard.ensure_root(plan)?;
    if plan.schema != SCHEMA
        || plan.parents.len() != plan.path.split('/').count() - 1
        || plan.intended_length > MAX_FILE_BYTES
        || manifest_digest(plan)? != journal.plan_identity
        || guard.io(active.identity())? != journal.transaction
        || !matches_intent(plan, &journal.staged)
        || plan.attributes.mode & !0o777 != 0
        || plan
            .expected
            .as_ref()
            .map_or(plan.attributes.mode != 0o600, |expected| {
                Attributes::of(expected) != plan.attributes
            })
        || plan
            .expected
            .as_ref()
            .is_some_and(|expected| expected.length > MAX_FILE_BYTES || expected.mode & !0o777 != 0)
    {
        return Err(guard.invalid("journal binding, schema or content identity mismatch"));
    }
    Ok(())
}

fn matches_intent(plan: &Manifest, staged: &Fingerprint) -> bool {
    staged.digest == plan.intended_digest
        && staged.length == plan.intended_length
        && Attributes::of(staged) == plan.attributes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    Batch {
        file: Option<usize>,
        phase: batch::Phase,
    },
    Record(RecordKind, fs::RecordPoint),
    BaselineChecked,
    StageSync,
    IntentDurable,
    Moved,
    BeforeInstall,
    InstallSync,
    Installed,
    BeforeRestore,
    RestoreSync,
    BeforeArchive,
    ArchiveSync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordKind {
    Ignore,
    Root,
    Intent,
    Restore,
}

#[cfg(test)]
// A fork briefly inherits unrelated flock descriptors until exec. Tests that
// create processes take the write guard; filesystem-only cases stay parallel.
static PROCESS_SPAWN: std::sync::RwLock<()> = std::sync::RwLock::new(());

#[cfg(test)]
mod tests;
