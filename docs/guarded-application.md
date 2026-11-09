# Guarded output application

`rebaze_bazel::apply` is an opt-in library API for Linux and macOS. It captures
destination state separately from pure generated intent and retains recovery
data when a write fails or an editor saves concurrently. Core migration still
uses the legacy per-file bundle writer. Batch application is also opt-in and
requires existing parent directories. Core integration and CLI recovery remain
R06 work; these APIs do not yet protect ordinary migrations.

## Ordinary writes while recovery is pending

The ordinary `write_bundle` / `write_files` path refuses any
`.rebaze-apply/active` entry before changing an output or creating a parent.
This includes empty directories, malformed journals, regular files and dangling
symlinks. A symlink or non-directory at `.rebaze-apply` also blocks writing;
inspection errors fail visibly. Completed archives alone do not block it.

Inspect the reported location and use the matching recovery API below. The
ordinary writer does not parse, delete or automatically recover pending state.
A successful recovery receipt still needs its outcome inspected. Never delete
`active` to make migration proceed without accounting for retained originals.

Native/JVM migration uses this preflight; a refusal fails its application stage.
Preview remains read-only and does not require recovery. This check does not
acquire the cooperative lock or prevent a guarded transaction starting afterward.
Callers must exclude concurrent writers. The external Bazelle writer has its
separate contract and is not protected by this preflight.

All `OutputPath` / `OutputBundle` constructors reject `.git`, `.hg`, `.svn` and
`.rebaze-apply` components, including ASCII case variants at any depth. Ordinary
configuration files such as `.bazelrc`, `.gitignore` and `.gitattributes` remain
valid output paths. This restriction protects both ordinary and guarded writers.

## Capture and apply

```rust
use rebaze_bazel::{OutputPath, apply::{FileEditPlan, MetadataPolicy}};
use std::path::Path;

let output = OutputPath::new("BUILD.bazel".to_owned())?;
let plan = FileEditPlan::capture(
    Path::new("/absolute/project"),
    &output,
    "# generated content\n".to_owned(),
    MetadataPolicy::PosixModeAndOwner,
)?;
let receipt = plan.apply()?;
// Inspect receipt.outcome and retain receipt.retained for recovery/audit.
# Ok::<(), Box<dyn std::error::Error>>(())
```

Capture reads the canonical root, existing parent directories and destination.
It performs no writes. Expected state includes absence or the regular file's
SHA-256, length, device/inode, mode, user and group. The owned plan cannot be
constructed by substituting a caller-provided baseline.

Application takes a nonblocking cooperative lock under `.rebaze-apply`. It
rejects a changed root, parent or baseline and refuses another pending
transaction. It stages and syncs intended bytes, verifies their digest and
metadata, then publishes immutable intent through a synced temporary record and
a no-clobber rename. Existing content is moved to `active/before` and the actual
moved entry is verified. A changed entry is restored without replacing a newly
created destination. Installation also uses a no-clobber rename.

Directory descriptors and component-by-component no-follow opens confine file
operations. Output paths cannot use `.rebaze-apply`, `.git`, `.hg` or `.svn`
components, including ASCII case variants. Parent directories must already exist.
Symlinks and nonregular files cannot be captured as a baseline. A symlink swapped
into the destination after capture can be moved/restored as an entry, without
following its referent.

## Recover an interruption

Call `rebaze_bazel::apply::recover(root)` with the same canonical project root.
It acquires the same lock and reconciles journal records with actual file
identities and bytes. Recovery checks the schema, root, parent identities,
transaction identity, plan digest and confined output path. Unknown fields,
malformed records and foreign bindings fail visibly. The digest detects
inconsistency; it is not authentication against someone who can rewrite the
project and its private journal as the same user.

| Outcome | Established fact |
| --- | --- |
| `Applied` | Intended bytes and selected metadata were observed at the destination. |
| `Restored` | The captured baseline, including expected absence, was observed again. |
| `RestoredConflict` | The entry actually moved aside changed or could not be verified; it was put back without clobbering a destination. |
| `Abandoned` | Staging stopped before durable intent; this transaction moved no output. The current baseline is unverified. |
| Error | State is unresolved; inspect the reported recovery location. Recovery refuses to overwrite an intervening destination. |
| `None` | No active transaction remains. This does not claim that an older applied output is unchanged. |

Completed transactions are moved to `.rebaze-apply/saved-<device>-<inode>`.
The metadata directory publishes its own `*` ignore rule before staging originals,
so new recovery data is excluded from ordinary Git adds. Already tracked files
remain the repository owner's responsibility. Originals, remaining staged bytes,
manifests and partial private record attempts
are retained even after success. The receipt returns that location; errors after
the archive rename report the archive rather than the former `active` path.
No automatic pruning or blind recursive deletion is performed. Inspect and
copy needed originals before deliberately removing an archive. Never remove
`active` merely to bypass an unresolved conflict.

Recovery can itself be interrupted and retried. A durable restore record
identifies the moved entry before restoration, so a later process can recognize
a completed restore. Record publication never exposes partial JSON: interrupted
temporary records are retained, and retries use another exclusive slot. Each
record has at most 64 attempts before an explicit inspection error.

## Stage and recover a batch

Pass a validated `OutputBundle` to `BatchEditPlan::capture(root, bundle, metadata)`.
Capture owns the generated bytes and observes every baseline without writes.
`apply()` acquires one root lock, checks all baselines, and publishes a preparation
record before staging. Every file is staged, synced and verified before the
complete batch intent is published and before any destination is moved.
Before preparation, a small compatibility marker is published and synced at
`manifest.json`. The known R05 single-file reader rejects that marker through
its strict decoder instead of treating a batch as missing single-file intent.
Batch recovery validates the exact bounded marker; unknown marker contents or
batch evidence without it fail visibly. This is a guard for that known reader,
not a general guarantee that older versions can recover newer journals. Finish
active recovery before changing library versions.

Use `recover_bundle(root)` for batch transactions and `recover(root)` for
single-file transactions. Each rejects the other's nonempty layout. An unknown,
mixed or malformed layout remains active with an error; recovery never silently
abandons a directory containing unfamiliar backups.

| Interrupted state | Recovery action |
| --- | --- |
| Empty active directory, unpublished marker attempts, or valid marker with unpublished preparation attempts | Archive as `Unprepared`; no output paths or verified-baseline claim are available. |
| Published preparation, incomplete staging | Check every captured destination; archive as `BaselineVerified` only when all still match. A changed or unreadable baseline remains pending. |
| Complete intent, unchanged baselines or verified unfinished moves | Resume forward installation with no-clobber renames. |
| Intended output observed after an interrupted rename | Sync the file and containing directories before acknowledging it. A sync failure leaves the batch pending. |
| Changed destination, staged bytes or moved entry | Stop forward application and retain all recovery data. Restore only unfinished moves or recorded restorations without replacing a destination. |
| Output installed and subsequently deleted | Preserve its absence, report conflict, and retain the original backup. |
| Durable stop or restoration intent | Continue conflict settlement; never resume forward application. |

The `BatchReceipt` distinguishes `Applied`, `BaselineVerified`, `Unprepared` and
`Conflict`. Its ordered `outputs` records current observations for each path:
applied, baseline, or a typed conflict reason. Earlier successful installations
are checked again; a durable completion hint does not prove a file is unchanged.
All-applied requires a final check of every target and its parents. These are
sequential observations, not an atomic snapshot that prevents future edits.

A conflict can leave an applied prefix, unchanged later files, restored entries,
or user changes. This is an explicit terminal conflict, not automatic whole-batch
undo. A user edit or recreated path wins. An IO error leaves recovery pending
unless it happened after archive rename, in which case the error names the saved
location. Inspect the receipt and retained originals before making further edits.
No recursive deletion or pruning is performed.

Batch limits are 128 files, 64 MiB total intended content, 128 MiB total captured
baseline content, and 2 MiB per batch record. Single-file/path limits below also
apply. Entry counts and record-publication attempts are bounded. These limits
apply equally to capture and recovery; invalid journal data cannot bypass them.
No new dependencies or external tools are needed for batch recovery.

Both APIs require the explicit metadata policy below. Missing parent creation,
safe default metadata integration, native/JVM migration routing, post-build
recovery reporting and the separate external Bazelle writer remain R06 follow-up
work. Do not infer transaction safety for those existing writer paths.

## Metadata, filesystem and concurrency limits

The explicit metadata policy preserves ordinary permission bits, user and group
for an existing destination. Set-id/sticky files are rejected. New files use mode
`0600`, the effective user, and the effective group or a setgid destination
parent's group. The private journal uses mode `0700`; records and staged files
start at `0600`. Failure to preserve ownership is an error before replacement.

This policy does **not** preserve extended ACLs, xattrs, timestamps, hard-link
relationships or filesystem flags. Choose this API only when that limited
contract is appropriate. It must not silently become the default for arbitrary
existing files when batch integration lands.

The protocol requires local filesystems that support directory sync and atomic
same-filesystem no-clobber renames. Unsupported operations and cross-device
renames fail; no fallback to clobbering rename is permitted. Process interruption
tests establish process recovery, not firmware behavior during sudden power loss
or network-filesystem guarantees. Native Windows application is unsupported.

Cooperating rebaze operations serialize on the lock. Uncooperative file saves
are checked before moving, after moving and at completion; displaced content is
retained. A process holding an old file descriptor can continue changing the
archived inode. No application can freeze an uncooperative writer's future work.
Directory names and the private journal namespace must remain stable during
individual filesystem operations. Identity checks reject observed parent/root
changes, but arbitrary hostile directory relocation by the same user is outside
the contract. No observer-atomic multi-file update is claimed.

Files and intended content are capped at 32 MiB; recovery records at 64 KiB;
output paths at 4096 bytes and 64 components. Reads are streamed and reject
changing observations. These are the single-file protocol's limits, not the
repository-wide input/process budgets planned in R07/R08.

## Validation

Run the ordinary protocol tests without external tools:

```bash
cargo test --locked -p rebaze-bazel apply::
```

The suite drives real filesystem operations through deterministic fault points
and starts fresh test processes that exit before destructors. It covers stale
baselines, same-length concurrent saves, recreated destinations, staged content
and permission changes, symlink swaps, malformed journals, record publication,
sync/install boundaries, archive errors, lock contention and interrupted recovery.
Boundary cases include a legitimate 32 MiB baseline and rejection just above it.
Batch cases include mixed existing/new files, failure of each stage/install,
partial staging with disk-full injection, record publication and restoration
interruption, current-prefix edits and deletion, recovery-side sync failure, and
exact/above-limit counts and aggregate bytes. Fresh processes exercise application
interruption followed by independent recovery.
The transition table drives the implementation; it is not a separate model-only
proof. Optional native CMake and corpus tools are not invoked by these tests.
