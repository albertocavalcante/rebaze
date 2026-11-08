#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;

use super::fs::MAX_RECORD_BYTES;
use super::*;

// A fork can briefly retain another thread's flock descriptor before exec
// closes CLOEXEC handles. Keep subprocess creation outside other tests' lock
// lifetimes; ordinary filesystem tests remain parallel under a shared guard.
static PROCESS_SPAWN: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn plan(root: &Path, content: &str) -> FileEditPlan {
    FileEditPlan::capture(
        root,
        &OutputPath::new("BUILD.bazel".into()).unwrap(),
        content.into(),
        MetadataPolicy::PosixModeAndOwner,
    )
    .unwrap()
}

fn assert_conflict(result: &Result<Option<ApplyReceipt>, ApplyError>) {
    assert!(
        matches!(result, Err(ApplyError::Conflict { .. })),
        "{result:?}"
    );
}

fn fail_at(point: Point) -> impl FnMut(Point) -> io::Result<()> {
    move |observed| {
        if observed == point {
            Err(io::Error::other("injected failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn applies_absent_and_existing_files_and_retains_original_mode_and_bytes() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let first = plan(root.path(), "first").apply().unwrap();
    assert_eq!(first.outcome, ApplyOutcome::Applied);
    assert_eq!(
        fs::read_to_string(root.path().join("BUILD.bazel")).unwrap(),
        "first"
    );
    assert!(first.retained.join("manifest.json").is_file());
    assert_eq!(
        fs::read(root.path().join(".rebaze-apply/.gitignore")).unwrap(),
        b"*\n"
    );
    assert!(recover(root.path()).unwrap().is_none());
    fs::set_permissions(
        root.path().join("BUILD.bazel"),
        Permissions::from_mode(0o640),
    )
    .unwrap();
    let before = fs::metadata(root.path().join("BUILD.bazel")).unwrap();
    let second = plan(root.path(), "second").apply().unwrap();
    let after = fs::metadata(root.path().join("BUILD.bazel")).unwrap();
    assert_eq!(
        fs::read_to_string(second.retained.join("before")).unwrap(),
        "first"
    );
    assert_eq!(after.mode() & 0o777, 0o640);
    assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
    assert_ne!(first.retained, second.retained);
}

#[test]
fn stale_baselines_and_absent_to_present_changes_do_not_overwrite() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for existing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("BUILD.bazel");
        if existing {
            fs::write(&destination, "old").unwrap();
        }
        let edit = plan(root.path(), "generated");
        fs::write(&destination, "user").unwrap();
        assert!(matches!(edit.apply(), Err(ApplyError::Conflict { .. })));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "user");
        assert!(recover(root.path()).unwrap().is_none());
    }
}

#[test]
fn same_length_in_place_save_between_check_and_move_is_restored() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("BUILD.bazel");
    fs::write(&destination, "old!").unwrap();
    let edit = plan(root.path(), "generated");
    let receipt = edit
        .apply_with(&mut |point| {
            if point == Point::BaselineChecked {
                fs::write(&destination, "user")?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt.outcome, ApplyOutcome::RestoredConflict);
    assert_eq!(fs::read_to_string(destination).unwrap(), "user");
    assert_eq!(
        fs::read_to_string(receipt.retained.join("after")).unwrap(),
        "generated"
    );
}

#[test]
fn recreated_destination_is_never_clobbered_and_backup_remains_recoverable() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for existing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("BUILD.bazel");
        if existing {
            fs::write(&destination, "baseline").unwrap();
        }
        let error = plan(root.path(), "generated")
            .apply_with(&mut |point| {
                if point == Point::BeforeInstall {
                    fs::write(&destination, "user")?;
                }
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(error, ApplyError::Io { .. }));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "user");
        assert_eq!(
            fs::read_to_string(root.path().join(".rebaze-apply/active/before")).ok(),
            existing.then(|| "baseline".to_owned())
        );
        assert_conflict(&recover(root.path()));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "user");
        // User explicitly removes the conflicting entry; recovery may now restore.
        fs::remove_file(&destination).unwrap();
        assert_eq!(
            recover(root.path()).unwrap().unwrap().outcome,
            ApplyOutcome::Restored
        );
        assert_eq!(
            fs::read_to_string(&destination).ok(),
            existing.then(|| "baseline".to_owned())
        );
        assert!(recover(root.path()).unwrap().is_none());
    }
}

#[test]
fn interrupted_recovery_of_a_changed_backup_is_idempotent() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("BUILD.bazel");
    fs::write(&destination, "old!").unwrap();
    let result = plan(root.path(), "generated").apply_with(&mut |point| {
        if point == Point::BaselineChecked {
            fs::write(&destination, "user")?;
        }
        if point == Point::RestoreSync {
            return Err(io::Error::other("interrupted restore"));
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "user");
    assert_eq!(
        recover(root.path()).unwrap().unwrap().outcome,
        ApplyOutcome::RestoredConflict
    );
    assert!(recover(root.path()).unwrap().is_none());
    assert_eq!(fs::read_to_string(destination).unwrap(), "user");
}

#[test]
fn recreation_during_restoration_preserves_both_user_versions() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("BUILD.bazel");
    fs::write(&destination, "baseline").unwrap();
    let result = plan(root.path(), "generated").apply_with(&mut |point| {
        if point == Point::BaselineChecked {
            fs::write(&destination, "first save")?;
        }
        if point == Point::BeforeRestore {
            fs::write(&destination, "second save")?;
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&destination).unwrap(), "second save");
    assert_eq!(
        fs::read_to_string(root.path().join(".rebaze-apply/active/before")).unwrap(),
        "first save"
    );
    assert_conflict(&recover(root.path()));
    assert_eq!(fs::read_to_string(&destination).unwrap(), "second save");
}

#[test]
fn faults_at_each_io_boundary_leave_a_conservative_terminal_state() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    // A small transition model: before install recover the baseline; at/after
    // install retain intended bytes. Drive the actual filesystem protocol.
    for existing in [false, true] {
        for point in [
            Point::StageSync,
            Point::IntentDurable,
            Point::Moved,
            Point::BeforeInstall,
            Point::InstallSync,
            Point::Installed,
            Point::BeforeArchive,
        ] {
            if !existing && point == Point::Moved {
                continue;
            }
            let root = tempfile::tempdir().unwrap();
            let destination = root.path().join("BUILD.bazel");
            if existing {
                fs::write(&destination, "baseline").unwrap();
            }
            assert!(
                plan(root.path(), "generated")
                    .apply_with(&mut fail_at(point))
                    .is_err()
            );
            let receipt = recover(root.path()).unwrap().unwrap();
            let installed = matches!(
                point,
                Point::InstallSync | Point::Installed | Point::BeforeArchive
            );
            assert_eq!(
                receipt.outcome,
                if point == Point::StageSync {
                    ApplyOutcome::Abandoned
                } else if installed {
                    ApplyOutcome::Applied
                } else {
                    ApplyOutcome::Restored
                },
                "{point:?}"
            );
            assert_eq!(
                fs::read_to_string(&destination).ok(),
                if installed {
                    Some("generated".into())
                } else if existing {
                    Some("baseline".into())
                } else {
                    None
                },
                "{point:?}"
            );
            assert!(recover(root.path()).unwrap().is_none());
        }
    }
}

#[test]
fn symlink_swaps_never_modify_the_referent() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let protected = outside.path().join("protected");
    fs::write(&protected, "outside").unwrap();
    let destination = root.path().join("BUILD.bazel");
    fs::write(&destination, "old").unwrap();
    let receipt = plan(root.path(), "generated")
        .apply_with(&mut |point| {
            if point == Point::BaselineChecked {
                fs::remove_file(&destination)?;
                symlink(&protected, &destination)?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt.outcome, ApplyOutcome::RestoredConflict);
    assert_eq!(fs::read_link(&destination).unwrap(), protected);
    assert_eq!(fs::read_to_string(&protected).unwrap(), "outside");
    assert!(
        FileEditPlan::capture(
            root.path(),
            &OutputPath::new("BUILD.bazel".into()).unwrap(),
            "bad".into(),
            MetadataPolicy::PosixModeAndOwner
        )
        .is_err()
    );
}

#[test]
fn changed_parent_and_private_directory_symlinks_are_rejected() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    let edit = FileEditPlan::capture(
        root.path(),
        &OutputPath::new("child/BUILD.bazel".into()).unwrap(),
        "generated".into(),
        MetadataPolicy::PosixModeAndOwner,
    )
    .unwrap();
    fs::rename(root.path().join("child"), root.path().join("saved-child")).unwrap();
    symlink(outside.path(), root.path().join("child")).unwrap();
    assert!(edit.apply().is_err());
    assert!(!outside.path().join("BUILD.bazel").exists());
    assert!(!root.path().join("saved-child/BUILD.bazel").exists());
    fs::remove_dir_all(root.path().join(".rebaze-apply")).unwrap();
    symlink(outside.path(), root.path().join(".rebaze-apply")).unwrap();
    assert!(plan(root.path(), "generated").apply().is_err());
    assert!(!outside.path().join("lock").exists());
}

#[test]
fn replaced_root_is_rejected_before_creating_private_metadata() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for symlink_root in [false, true] {
        let container = tempfile::tempdir().unwrap();
        let root = container.path().join("project");
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(&root).unwrap();
        let edit = plan(&root, "generated");
        fs::rename(&root, container.path().join("previous")).unwrap();
        if symlink_root {
            symlink(outside.path(), &root).unwrap();
        } else {
            fs::create_dir(&root).unwrap();
        }
        assert!(matches!(edit.apply(), Err(ApplyError::Invalid(_))));
        assert!(!root.join(".rebaze-apply").exists());
        assert!(!outside.path().join(".rebaze-apply").exists());
        assert!(!container.path().join("previous/.rebaze-apply").exists());
    }
}

#[test]
fn exposed_metadata_directory_and_hard_linked_lock_are_rejected() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(".rebaze-apply");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, Permissions::from_mode(0o755)).unwrap();
    assert!(plan(root.path(), "generated").apply().is_err());
    assert!(!directory.join("lock").exists());
    fs::set_permissions(&directory, Permissions::from_mode(0o700)).unwrap();
    let outside = root.path().join("protected");
    fs::write(&outside, "outside").unwrap();
    fs::hard_link(&outside, directory.join("lock")).unwrap();
    assert!(plan(root.path(), "generated").apply().is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
    assert!(!root.path().join("BUILD.bazel").exists());
}

#[test]
fn malformed_escaping_foreign_and_oversized_journals_never_write_destinations() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for mutation in [
        "malformed",
        "escape",
        "foreign",
        "schema",
        "unknown",
        "oversized",
        "digest",
    ] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("BUILD.bazel");
        fs::write(&destination, "baseline").unwrap();
        assert!(
            plan(root.path(), "generated")
                .apply_with(&mut fail_at(Point::IntentDurable))
                .is_err()
        );
        let path = root.path().join(".rebaze-apply/active/manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mutation {
            "malformed" => {
                fs::write(&path, "{").unwrap();
            }
            "oversized" => {
                fs::write(
                    &path,
                    vec![b' '; usize::try_from(MAX_RECORD_BYTES).unwrap() + 1],
                )
                .unwrap();
            }
            _ => {
                match mutation {
                    "escape" => value["plan"]["path"] = "../outside".into(),
                    "foreign" => value["plan"]["root"] = "/different-root".into(),
                    "schema" => value["plan"]["schema"] = 99.into(),
                    "unknown" => value["extra"] = true.into(),
                    "digest" => value["plan"]["intended_digest"][0] = 255.into(),
                    _ => unreachable!(),
                }
                fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
        }
        assert!(recover(root.path()).is_err(), "{mutation}");
        assert_eq!(fs::read_to_string(destination).unwrap(), "baseline");
        assert_eq!(
            fs::read_to_string(root.path().join(".rebaze-apply/active/after")).unwrap(),
            "generated"
        );
    }
}

#[test]
fn cooperative_lock_rejects_overlap_and_pending_intent_blocks_new_apply() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let guard = Guard::open(root.path()).unwrap();
    assert!(matches!(
        plan(root.path(), "generated").apply(),
        Err(ApplyError::Locked)
    ));
    drop(guard);
    assert!(
        plan(root.path(), "generated")
            .apply_with(&mut fail_at(Point::IntentDurable))
            .is_err()
    );
    let result = plan(root.path(), "other").apply();
    assert!(
        matches!(result, Err(ApplyError::Conflict { .. })),
        "{result:?}"
    );
    recover(root.path()).unwrap();
}

#[test]
fn input_limits_and_reserved_names_are_explicit() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    for path in [".rebaze-apply/file", ".GIT/file", "child/.svn/file"] {
        assert!(
            FileEditPlan::capture(
                root.path(),
                &OutputPath::new(path.into()).unwrap(),
                "text".into(),
                MetadataPolicy::PosixModeAndOwner
            )
            .is_err()
        );
    }
    assert!(
        FileEditPlan::capture(
            root.path(),
            &OutputPath::new("BUILD.bazel".into()).unwrap(),
            "x".repeat(usize::try_from(MAX_FILE_BYTES).unwrap() + 1),
            MetadataPolicy::PosixModeAndOwner
        )
        .is_err()
    );
    let destination = root.path().join("BUILD.bazel");
    File::create(&destination)
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    assert!(
        FileEditPlan::capture(
            root.path(),
            &OutputPath::new("BUILD.bazel".into()).unwrap(),
            "text".into(),
            MetadataPolicy::PosixModeAndOwner
        )
        .is_err()
    );
    File::create(&destination)
        .unwrap()
        .set_len(MAX_FILE_BYTES)
        .unwrap();
    assert!(
        FileEditPlan::capture(
            root.path(),
            &OutputPath::new("BUILD.bazel".into()).unwrap(),
            "text".into(),
            MetadataPolicy::PosixModeAndOwner
        )
        .is_ok()
    );
}

#[test]
fn staged_bytes_and_metadata_must_match_the_captured_plan() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for existing in [false, true] {
        for metadata_only in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let destination = root.path().join("BUILD.bazel");
            if existing {
                fs::write(&destination, "baseline").unwrap();
            }
            let error = plan(root.path(), "generated")
                .apply_with(&mut |point| {
                    if point == Point::StageSync {
                        let staged = root.path().join(".rebaze-apply/active/after");
                        if metadata_only {
                            fs::set_permissions(staged, Permissions::from_mode(0o777))?;
                        } else {
                            fs::write(staged, "corrupted")?;
                        }
                    }
                    Ok(())
                })
                .unwrap_err();
            assert!(matches!(error, ApplyError::Conflict { .. }));
            assert_eq!(
                fs::read_to_string(&destination).ok(),
                existing.then(|| "baseline".to_owned())
            );
            assert_eq!(
                recover(root.path()).unwrap().unwrap().outcome,
                ApplyOutcome::Abandoned
            );
        }
    }
}

#[test]
fn archive_sync_failure_reports_the_actual_retained_directory() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("BUILD.bazel"), "baseline").unwrap();
    let error = plan(root.path(), "generated")
        .apply_with(&mut fail_at(Point::ArchiveSync))
        .unwrap_err();
    let ApplyError::Io { retained, .. } = error else {
        panic!("expected IO failure")
    };
    assert!(retained.is_dir());
    assert_ne!(retained.file_name().unwrap(), "active");
    assert_eq!(
        fs::read_to_string(retained.join("before")).unwrap(),
        "baseline"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("BUILD.bazel")).unwrap(),
        "generated"
    );
    assert!(recover(root.path()).unwrap().is_none());
}

#[test]
fn an_unprepared_transaction_does_not_claim_to_restore_an_intervening_edit() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("BUILD.bazel"), "baseline").unwrap();
    let result = plan(root.path(), "generated").apply_with(&mut |point| {
        if point == Point::StageSync {
            fs::write(root.path().join("BUILD.bazel"), "user")?;
            return Err(io::Error::other("stage sync failed"));
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        recover(root.path()).unwrap().unwrap().outcome,
        ApplyOutcome::Abandoned
    );
    assert_eq!(
        fs::read_to_string(root.path().join("BUILD.bazel")).unwrap(),
        "user"
    );
}

#[test]
fn interrupted_record_publication_keeps_partial_json_private_and_retryable() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for kind in [
        RecordKind::Ignore,
        RecordKind::Root,
        RecordKind::Intent,
        RecordKind::Restore,
    ] {
        for selected in [
            super::fs::RecordPoint::Created,
            super::fs::RecordPoint::Written,
            super::fs::RecordPoint::Synced,
            super::fs::RecordPoint::Published,
        ] {
            let root = tempfile::tempdir().unwrap();
            let destination = root.path().join("BUILD.bazel");
            fs::write(&destination, "baseline").unwrap();
            let result = plan(root.path(), "generated").apply_with(&mut |point| {
                if kind == RecordKind::Restore && point == Point::BaselineChecked {
                    fs::write(&destination, "useredit")?;
                }
                if point == Point::Record(kind, selected) {
                    return Err(io::Error::other("record publication interrupted"));
                }
                Ok(())
            });
            assert!(result.is_err());
            let receipt = recover(root.path()).unwrap();
            let expected_outcome = match kind {
                RecordKind::Ignore | RecordKind::Root => None,
                RecordKind::Intent => Some(if selected == super::fs::RecordPoint::Published {
                    ApplyOutcome::Restored
                } else {
                    ApplyOutcome::Abandoned
                }),
                RecordKind::Restore => Some(ApplyOutcome::RestoredConflict),
            };
            assert_eq!(
                receipt.map(|value| value.outcome),
                expected_outcome,
                "{kind:?} {selected:?}"
            );
            assert_eq!(
                fs::read_to_string(&destination).unwrap(),
                if kind == RecordKind::Restore {
                    "useredit"
                } else {
                    "baseline"
                }
            );
            assert!(recover(root.path()).unwrap().is_none());
        }
    }
}

#[test]
fn record_attempt_bound_accepts_the_last_slot_and_preserves_exhausted_attempts() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for occupied in [63, 64] {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join(".rebaze-apply");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, Permissions::from_mode(0o700)).unwrap();
        fs::write(root.path().join("BUILD.bazel"), "baseline").unwrap();
        for attempt in 0..occupied {
            fs::write(
                directory.join(format!(".pending-root.json-{attempt}")),
                "partial private record",
            )
            .unwrap();
        }
        let result = recover(root.path());
        if occupied == 63 {
            assert_eq!(result.unwrap(), None);
            assert!(directory.join("root.json").is_file());
        } else {
            let ApplyError::Io { retained, source } = result.unwrap_err() else {
                panic!("expected IO failure")
            };
            assert_eq!(retained, directory);
            assert!(
                source
                    .to_string()
                    .contains("too many interrupted record attempts")
            );
            assert!(!directory.join("root.json").exists());
        }
        assert_eq!(
            fs::read_to_string(root.path().join("BUILD.bazel")).unwrap(),
            "baseline"
        );
        assert!(!directory.join("active").exists());
        for attempt in 0..occupied {
            assert_eq!(
                fs::read_to_string(directory.join(format!(".pending-root.json-{attempt}")))
                    .unwrap(),
                "partial private record"
            );
        }
    }
}

#[test]
fn real_process_interruption_releases_lock_and_recovers() {
    let _spawn_guard = PROCESS_SPAWN.write().unwrap();
    let cases = [
        ("intent", Some(ApplyOutcome::Restored), "baseline"),
        ("moved", Some(ApplyOutcome::Restored), "baseline"),
        ("installed", Some(ApplyOutcome::Applied), "generated"),
        ("restore", Some(ApplyOutcome::RestoredConflict), "useredit"),
        ("root-record", None, "baseline"),
        ("intent-record", Some(ApplyOutcome::Abandoned), "baseline"),
        (
            "restore-record",
            Some(ApplyOutcome::RestoredConflict),
            "useredit",
        ),
        ("archive", None, "generated"),
        ("recovery", Some(ApplyOutcome::Restored), "baseline"),
    ];
    for (point, outcome, content) in cases {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("BUILD.bazel"), "baseline").unwrap();
        if point == "recovery" {
            assert!(
                plan(root.path(), "generated")
                    .apply_with(&mut fail_at(Point::Moved))
                    .is_err()
            );
        }
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "apply::tests::interruption_child", "--ignored"])
            .env("REBAZE_APPLY_TEST_ROOT", root.path())
            .env("REBAZE_APPLY_TEST_POINT", point)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(79),
            "{point}: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        let result = recover(root.path()).unwrap();
        assert_eq!(result.map(|value| value.outcome), outcome, "{point}");
        assert_eq!(
            fs::read_to_string(root.path().join("BUILD.bazel")).unwrap(),
            content,
            "{point}"
        );
        assert!(recover(root.path()).unwrap().is_none());
    }
}

#[test]
#[ignore = "subprocess helper, invoked explicitly by real_process_interruption_releases_lock_and_recovers"]
fn interruption_child() {
    let root = PathBuf::from(std::env::var_os("REBAZE_APPLY_TEST_ROOT").unwrap());
    let selected = std::env::var("REBAZE_APPLY_TEST_POINT").unwrap();
    let mut hook = |point| {
        if matches!(selected.as_str(), "restore" | "restore-record")
            && point == Point::BaselineChecked
        {
            fs::write(root.join("BUILD.bazel"), "useredit")?;
        }
        if matches!(
            (selected.as_str(), point),
            ("intent", Point::IntentDurable)
                | ("moved", Point::Moved)
                | ("installed", Point::Installed)
                | ("restore" | "recovery", Point::RestoreSync)
                | ("archive", Point::ArchiveSync)
                | (
                    "root-record",
                    Point::Record(RecordKind::Root, super::fs::RecordPoint::Created)
                )
                | (
                    "intent-record",
                    Point::Record(RecordKind::Intent, super::fs::RecordPoint::Written)
                )
                | (
                    "restore-record",
                    Point::Record(RecordKind::Restore, super::fs::RecordPoint::Synced)
                )
        ) {
            std::process::exit(79);
        }
        Ok(())
    };
    if selected == "recovery" {
        let _result = recover_with(&root, &mut hook);
    } else {
        let _result = plan(&root, "generated").apply_with(&mut hook);
    }
    panic!("selected interruption point was never reached");
}
