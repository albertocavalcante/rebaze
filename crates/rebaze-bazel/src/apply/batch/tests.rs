#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::process::Command;

use super::super::PROCESS_SPAWN;
use super::super::fs::RecordPoint;
use super::*;

fn bundle() -> OutputBundle {
    OutputBundle::new([
        ("a.bzl".into(), "new-a".into()),
        ("b.bzl".into(), "new-b".into()),
        ("child/c.bzl".into(), "new-c".into()),
    ])
    .unwrap()
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    fs::write(root.path().join("a.bzl"), "old-a").unwrap();
    fs::write(root.path().join("child/c.bzl"), "old-c").unwrap();
    root
}

fn plan(root: &Path) -> BatchEditPlan {
    BatchEditPlan::capture(root, bundle(), MetadataPolicy::PosixModeAndOwner).unwrap()
}

fn point(file: Option<usize>, phase: Phase) -> Point {
    Point::Batch { file, phase }
}

fn fail(selected: Point) -> impl FnMut(Point) -> io::Result<()> {
    move |seen| {
        if seen == selected {
            Err(io::Error::other("injected batch IO fault"))
        } else {
            Ok(())
        }
    }
}

fn baseline(root: &Path) {
    assert_eq!(fs::read_to_string(root.join("a.bzl")).unwrap(), "old-a");
    assert!(!root.join("b.bzl").exists());
    assert_eq!(
        fs::read_to_string(root.join("child/c.bzl")).unwrap(),
        "old-c"
    );
}

fn intended(root: &Path) {
    for (path, content) in bundle() {
        assert_eq!(
            fs::read_to_string(root.join(path.as_str())).unwrap(),
            content
        );
    }
}

#[test]
fn complete_staging_precedes_every_move_and_receipt_retains_originals() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    fs::set_permissions(root.path().join("a.bzl"), fs::Permissions::from_mode(0o640)).unwrap();
    let before = fs::metadata(root.path().join("a.bzl")).unwrap();
    let receipt = plan(root.path())
        .apply_with(&mut |seen| {
            if seen == point(None, Phase::IntentDurable) {
                baseline(root.path());
                for index in 0..3 {
                    assert!(
                        root.path()
                            .join(format!(".rebaze-apply/active/entries/{index:04}/after"))
                            .is_file()
                    );
                }
            }
            Ok(())
        })
        .unwrap();
    intended(root.path());
    assert_eq!(receipt.outcome, BatchOutcome::Applied);
    assert!(
        receipt
            .outputs
            .iter()
            .all(|entry| entry.disposition == OutputDisposition::Applied)
    );
    assert_eq!(
        fs::read_to_string(receipt.retained.join("entries/0000/before")).unwrap(),
        "old-a"
    );
    assert_eq!(
        fs::read_to_string(receipt.retained.join("entries/0002/before")).unwrap(),
        "old-c"
    );
    let after = fs::metadata(root.path().join("a.bzl")).unwrap();
    assert_eq!(after.mode() & 0o777, 0o640);
    assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
    assert!(recover_bundle(root.path()).unwrap().is_none());
}

#[test]
fn every_stage_failure_leaves_destinations_unchanged() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for index in 0..3 {
        for phase in [
            Phase::StageStarted,
            Phase::StageSync,
            Phase::StageDirectorySync,
        ] {
            let root = fixture();
            assert!(
                plan(root.path())
                    .apply_with(&mut fail(point(Some(index), phase)))
                    .is_err()
            );
            baseline(root.path());
            let receipt = recover_bundle(root.path()).unwrap().unwrap();
            assert_eq!(receipt.outcome, BatchOutcome::BaselineVerified);
            assert!(
                receipt
                    .outputs
                    .iter()
                    .all(|entry| entry.disposition == OutputDisposition::Baseline)
            );
            baseline(root.path());
        }
    }
}

#[test]
fn partial_stage_and_disk_full_are_not_misreported_as_applied() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    let result = plan(root.path()).apply_with(&mut |seen| {
        if seen == point(Some(1), Phase::StageStarted) {
            let mut file =
                File::create(root.path().join(".rebaze-apply/active/entries/0001/after"))?;
            file.write_all(b"partial")?;
            return Err(io::Error::from_raw_os_error(
                rustix::io::Errno::NOSPC.raw_os_error(),
            ));
        }
        Ok(())
    });
    assert!(matches!(result, Err(ApplyError::Output { .. })));
    baseline(root.path());
    let receipt = recover_bundle(root.path()).unwrap().unwrap();
    assert_eq!(receipt.outcome, BatchOutcome::BaselineVerified);
    assert_eq!(
        fs::read(receipt.retained.join("entries/0001/after")).unwrap(),
        b"partial"
    );
}

#[test]
fn interruption_at_each_forward_transition_recovers_all_intended_files() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let mut points = vec![point(None, Phase::IntentDurable), Point::BeforeArchive];
    for index in 0..3 {
        for phase in [
            Phase::BaselineChecked,
            Phase::BeforeInstall,
            Phase::InstallSync,
            Phase::Installed,
            Phase::DoneRecord(RecordPoint::Created),
            Phase::DoneRecord(RecordPoint::Written),
            Phase::DoneRecord(RecordPoint::Synced),
            Phase::DoneRecord(RecordPoint::Published),
        ] {
            points.push(point(Some(index), phase));
        }
        if index != 1 {
            points.push(point(Some(index), Phase::Moved));
        }
    }
    for selected in points {
        let root = fixture();
        assert!(
            plan(root.path()).apply_with(&mut fail(selected)).is_err(),
            "{selected:?}"
        );
        let receipt = recover_bundle(root.path()).unwrap().unwrap();
        assert_eq!(receipt.outcome, BatchOutcome::Applied, "{selected:?}");
        intended(root.path());
        assert!(recover_bundle(root.path()).unwrap().is_none());
    }
}

#[test]
fn intent_publication_is_the_boundary_between_staging_and_forward_recovery() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for record in [
        RecordPoint::Created,
        RecordPoint::Written,
        RecordPoint::Synced,
        RecordPoint::Published,
    ] {
        let root = fixture();
        assert!(
            plan(root.path())
                .apply_with(&mut fail(point(None, Phase::IntentRecord(record))))
                .is_err()
        );
        baseline(root.path());
        let receipt = recover_bundle(root.path()).unwrap().unwrap();
        if record == RecordPoint::Published {
            assert_eq!(receipt.outcome, BatchOutcome::Applied);
            intended(root.path());
        } else {
            assert_eq!(receipt.outcome, BatchOutcome::BaselineVerified);
            baseline(root.path());
        }
    }
}

#[test]
fn changed_later_file_does_not_leave_an_unverified_successful_prefix() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    let receipt = plan(root.path())
        .apply_with(&mut |seen| {
            if seen == point(Some(0), Phase::Installed) {
                fs::write(root.path().join("a.bzl"), "user-a")?;
                fs::write(root.path().join("child/c.bzl"), "user-c")?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt.outcome, BatchOutcome::Conflict);
    assert_eq!(
        receipt.outputs[0].disposition,
        OutputDisposition::Conflict(ConflictReason::DestinationChanged)
    );
    assert_eq!(
        receipt.outputs[2].disposition,
        OutputDisposition::Conflict(ConflictReason::DestinationChanged)
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.bzl")).unwrap(),
        "user-a"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("child/c.bzl")).unwrap(),
        "user-c"
    );
    assert_eq!(
        fs::read_to_string(receipt.retained.join("entries/0000/before")).unwrap(),
        "old-a"
    );
}

#[test]
fn final_verification_rechecks_earlier_installed_output() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    let receipt = plan(root.path())
        .apply_with(&mut |seen| {
            if seen == point(Some(2), Phase::Installed) {
                fs::write(root.path().join("a.bzl"), "later save")?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt.outcome, BatchOutcome::Conflict);
    assert_eq!(
        receipt.outputs[0].disposition,
        OutputDisposition::Conflict(ConflictReason::DestinationChanged)
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.bzl")).unwrap(),
        "later save"
    );
    assert_eq!(receipt.outputs[1].disposition, OutputDisposition::Applied);
    assert_eq!(receipt.outputs[2].disposition, OutputDisposition::Applied);
}

#[test]
fn save_between_check_and_move_is_restored_and_later_files_stay_baseline() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    let receipt = plan(root.path())
        .apply_with(&mut |seen| {
            if seen == point(Some(0), Phase::BaselineChecked) {
                fs::write(root.path().join("a.bzl"), "user!")?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(receipt.outcome, BatchOutcome::Conflict);
    assert_eq!(
        fs::read_to_string(root.path().join("a.bzl")).unwrap(),
        "user!"
    );
    assert!(!root.path().join("b.bzl").exists());
    assert_eq!(receipt.outputs[1].disposition, OutputDisposition::Baseline);
    assert_eq!(receipt.outputs[2].disposition, OutputDisposition::Baseline);
    assert_eq!(
        fs::read_to_string(receipt.retained.join("entries/0000/after")).unwrap(),
        "new-a"
    );
}

#[test]
fn recreated_destination_preserves_both_user_entry_and_baseline_backup() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for index in 0..3 {
        let root = fixture();
        let path = bundle().iter().nth(index).unwrap().0.clone();
        assert!(
            plan(root.path())
                .apply_with(&mut |seen| {
                    if seen == point(Some(index), Phase::BeforeInstall) {
                        fs::write(root.path().join(path.as_str()), "user")?;
                    }
                    Ok(())
                })
                .is_err()
        );
        let receipt = recover_bundle(root.path()).unwrap().unwrap();
        assert_eq!(receipt.outcome, BatchOutcome::Conflict);
        assert_eq!(
            fs::read_to_string(root.path().join(path.as_str())).unwrap(),
            "user"
        );
        assert_eq!(
            receipt.outputs[index].disposition,
            OutputDisposition::Conflict(ConflictReason::DestinationChanged)
        );
        if index != 1 {
            assert!(
                receipt
                    .retained
                    .join(format!("entries/{index:04}/before"))
                    .is_file()
            );
        }
    }
}

#[test]
fn restoration_intent_prevents_reapplication_after_every_restore_fault() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for phase in [
        Phase::StopRecord(RecordPoint::Created),
        Phase::StopRecord(RecordPoint::Written),
        Phase::StopRecord(RecordPoint::Synced),
        Phase::StopRecord(RecordPoint::Published),
        Phase::RestoreRecord(RecordPoint::Created),
        Phase::RestoreRecord(RecordPoint::Written),
        Phase::RestoreRecord(RecordPoint::Synced),
        Phase::RestoreRecord(RecordPoint::Published),
        Phase::BeforeRestore,
        Phase::RestoreSync,
    ] {
        let root = fixture();
        assert!(
            plan(root.path())
                .apply_with(&mut |seen| {
                    if seen == point(Some(0), Phase::BaselineChecked) {
                        fs::write(root.path().join("a.bzl"), "saved")?;
                    }
                    let selected = if matches!(phase, Phase::StopRecord(_)) {
                        None
                    } else {
                        Some(0)
                    };
                    fail(point(selected, phase))(seen)
                })
                .is_err()
        );
        let receipt = recover_bundle(root.path()).unwrap().unwrap();
        assert_eq!(receipt.outcome, BatchOutcome::Conflict, "{phase:?}");
        assert_eq!(
            fs::read_to_string(root.path().join("a.bzl")).unwrap(),
            "saved",
            "{phase:?}"
        );
        assert!(!root.path().join("b.bzl").exists());
        assert!(recover_bundle(root.path()).unwrap().is_none());
    }
}

#[test]
fn symlink_save_and_recreation_during_restore_never_touch_the_referent() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for recreate in [false, true] {
        let root = fixture();
        let outside = tempfile::tempdir().unwrap();
        let protected = outside.path().join("protected");
        fs::write(&protected, "outside").unwrap();
        let result = plan(root.path()).apply_with(&mut |seen| {
            if seen == point(Some(0), Phase::BaselineChecked) {
                fs::remove_file(root.path().join("a.bzl"))?;
                symlink(&protected, root.path().join("a.bzl"))?;
            }
            if recreate && seen == point(Some(0), Phase::BeforeRestore) {
                fs::write(root.path().join("a.bzl"), "second save")?;
            }
            Ok(())
        });
        let receipt = if recreate {
            assert!(result.is_err());
            recover_bundle(root.path()).unwrap().unwrap()
        } else {
            result.unwrap()
        };
        assert_eq!(receipt.outcome, BatchOutcome::Conflict);
        assert_eq!(fs::read_to_string(&protected).unwrap(), "outside");
        if recreate {
            assert_eq!(
                fs::read_to_string(root.path().join("a.bzl")).unwrap(),
                "second save"
            );
            assert_eq!(
                fs::read_link(receipt.retained.join("entries/0000/before")).unwrap(),
                protected
            );
        } else {
            assert_eq!(fs::read_link(root.path().join("a.bzl")).unwrap(), protected);
        }
    }
}

#[test]
fn malformed_foreign_ambiguous_and_missing_intent_never_changes_destinations() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for mutation in [
        "malformed",
        "foreign",
        "path",
        "duplicate",
        "order",
        "schema",
        "digest",
        "unknown",
        "missing-preparation",
        "missing-complete-backup",
        "oversized",
        "extra-entry",
    ] {
        let root = fixture();
        assert!(
            plan(root.path())
                .apply_with(&mut fail(point(None, Phase::IntentDurable)))
                .is_err()
        );
        let active = root.path().join(".rebaze-apply/active");
        let path = active.join("batch-plan.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mutation {
            "malformed" => fs::write(&path, "{").unwrap(),
            "oversized" => {
                File::create(&path)
                    .unwrap()
                    .set_len(MAX_INTENT_BYTES + 1)
                    .unwrap();
            }
            "missing-preparation" => fs::remove_file(&path).unwrap(),
            "missing-complete-backup" => {
                fs::remove_file(active.join("batch.json")).unwrap();
                fs::rename(
                    root.path().join("a.bzl"),
                    active.join("entries/0000/before"),
                )
                .unwrap();
            }
            "extra-entry" => fs::create_dir(active.join("entries/9999")).unwrap(),
            _ => {
                match mutation {
                    "foreign" => value["plans"][0]["root"] = "/foreign".into(),
                    "path" => value["plans"][0]["path"] = "../escape".into(),
                    "duplicate" => value["plans"][1] = value["plans"][0].clone(),
                    "order" => value["plans"].as_array_mut().unwrap().swap(0, 1),
                    "schema" => value["schema"] = 99.into(),
                    "digest" => value["identity"][0] = 256.into(),
                    "unknown" => value["extra"] = true.into(),
                    _ => unreachable!(),
                }
                fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
        }
        assert!(recover_bundle(root.path()).is_err(), "{mutation}");
        if mutation != "missing-complete-backup" {
            baseline(root.path());
        } else {
            assert_eq!(
                fs::read(active.join("entries/0000/before")).unwrap(),
                b"old-a"
            );
        }
        assert!(active.is_dir());
    }
}

#[test]
fn interrupted_staging_refuses_a_baseline_claim_after_user_edits() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    assert!(
        plan(root.path())
            .apply_with(&mut fail(point(Some(1), Phase::StageSync)))
            .is_err()
    );
    fs::write(root.path().join("a.bzl"), "user").unwrap();
    assert!(recover_bundle(root.path()).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("a.bzl")).unwrap(),
        "user"
    );
    assert!(root.path().join(".rebaze-apply/active").is_dir());
}

#[test]
fn stale_baselines_and_replaced_parents_fail_before_any_output_changes() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    let edit = plan(root.path());
    fs::write(root.path().join("child/c.bzl"), "user").unwrap();
    assert!(edit.apply().is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("a.bzl")).unwrap(),
        "old-a"
    );
    assert!(!root.path().join(".rebaze-apply/active").exists());
    let edit = plan(root.path());
    fs::rename(
        root.path().join("child"),
        root.path().join("original-child"),
    )
    .unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    assert!(edit.apply().is_err());
    assert!(!root.path().join("b.bzl").exists());
    assert!(!root.path().join("child/c.bzl").exists());
}

#[test]
fn file_count_limit_accepts_the_boundary_and_rejects_the_next() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for count in [MAX_FILES, MAX_FILES + 1] {
        let root = tempfile::tempdir().unwrap();
        let bundle =
            OutputBundle::new((0..count).map(|index| (format!("file-{index:04}"), "ok".into())))
                .unwrap();
        let captured =
            BatchEditPlan::capture(root.path(), bundle, MetadataPolicy::PosixModeAndOwner);
        if count == MAX_FILES {
            let receipt = captured.unwrap().apply().unwrap();
            assert_eq!(receipt.outputs.len(), count);
            assert_eq!(receipt.outcome, BatchOutcome::Applied);
        } else {
            assert!(captured.is_err());
            assert!(!root.path().join(".rebaze-apply").exists());
        }
    }
}

#[test]
fn aggregate_output_and_baseline_limits_accept_exact_boundaries() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = tempfile::tempdir().unwrap();
    let per_file = usize::try_from(super::super::fs::MAX_FILE_BYTES).unwrap();
    let outputs = OutputBundle::new([
        ("a".into(), "x".repeat(per_file)),
        ("b".into(), "y".repeat(per_file)),
    ])
    .unwrap();
    // Capture must accept a legitimate 64 MiB output bundle, not just reject
    // oversized inputs. No filesystem observations are fabricated by the test.
    let captured =
        BatchEditPlan::capture(root.path(), outputs, MetadataPolicy::PosixModeAndOwner).unwrap();
    assert_eq!(
        captured
            .files
            .iter()
            .map(|file| file.content.len() as u64)
            .sum::<u64>(),
        MAX_OUTPUT_BYTES
    );
    drop(captured);
    let outputs = OutputBundle::new([
        ("a".into(), "x".repeat(per_file)),
        ("b".into(), "y".repeat(per_file)),
        ("c".into(), "z".into()),
    ])
    .unwrap();
    assert!(
        BatchEditPlan::capture(root.path(), outputs, MetadataPolicy::PosixModeAndOwner).is_err()
    );
    assert!(!root.path().join(".rebaze-apply").exists());
    for index in 0..4 {
        File::create(root.path().join(format!("baseline-{index}")))
            .unwrap()
            .set_len(per_file as u64)
            .unwrap();
    }
    File::create(root.path().join("baseline-4")).unwrap();
    for extra in [0, 1] {
        File::options()
            .write(true)
            .open(root.path().join("baseline-4"))
            .unwrap()
            .set_len(extra)
            .unwrap();
        let outputs =
            OutputBundle::new((0..5).map(|index| (format!("baseline-{index}"), "ok".into())))
                .unwrap();
        let captured =
            BatchEditPlan::capture(root.path(), outputs, MetadataPolicy::PosixModeAndOwner);
        if extra == 0 {
            assert_eq!(captured.unwrap().files.len(), 5);
        } else {
            assert!(captured.is_err());
        }
    }
}

#[test]
fn record_byte_limit_accepts_valid_json_at_limit_and_preserves_oversized_intent() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for extra in [0, 1] {
        let root = fixture();
        assert!(
            plan(root.path())
                .apply_with(&mut fail(point(None, Phase::IntentDurable)))
                .is_err()
        );
        let path = root.path().join(".rebaze-apply/active/batch.json");
        let mut bytes = fs::read(&path).unwrap();
        bytes.resize(usize::try_from(MAX_INTENT_BYTES).unwrap() + extra, b' ');
        fs::write(&path, bytes).unwrap();
        let recovered = recover_bundle(root.path());
        if extra == 0 {
            assert_eq!(recovered.unwrap().unwrap().outcome, BatchOutcome::Applied);
            intended(root.path());
        } else {
            assert!(recovered.is_err());
            baseline(root.path());
            assert!(path.is_file());
        }
    }
}

#[test]
fn corrupted_later_stage_is_detected_before_any_forward_recovery_write() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    assert!(
        plan(root.path())
            .apply_with(&mut fail(point(None, Phase::IntentDurable)))
            .is_err()
    );
    fs::write(
        root.path().join(".rebaze-apply/active/entries/0002/after"),
        "corrupt",
    )
    .unwrap();
    let receipt = recover_bundle(root.path()).unwrap().unwrap();
    assert_eq!(receipt.outcome, BatchOutcome::Conflict);
    baseline(root.path());
    assert!(receipt.retained.join("entries/0000/after").is_file());
    assert_eq!(
        receipt.outputs[2].disposition,
        OutputDisposition::Conflict(ConflictReason::MissingStage)
    );
}

#[test]
fn internally_consistent_checksums_do_not_bypass_path_and_binding_validation() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for mutation in [
        "duplicate",
        "case-alias",
        "ancestor",
        "order",
        "escape",
        "foreign-root",
        "schema",
        "per-file-limit",
    ] {
        let root = fixture();
        assert!(
            plan(root.path())
                .apply_with(&mut fail(point(None, Phase::IntentDurable)))
                .is_err()
        );
        let path = root.path().join(".rebaze-apply/active/batch-plan.json");
        let mut preparation: Preparation =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mutation {
            "duplicate" => preparation.plans[1].path = preparation.plans[0].path.clone(),
            "case-alias" => preparation.plans[1].path = "A.BZL".into(),
            "ancestor" => preparation.plans[1].path = "a.bzl/descendant".into(),
            "order" => preparation.plans.swap(0, 1),
            "escape" => preparation.plans[0].path = "../outside".into(),
            "foreign-root" => preparation.plans[0].root = "/foreign-root".into(),
            "schema" => preparation.plans[0].schema = 99,
            "per-file-limit" => {
                preparation.plans[0].intended_length = super::super::fs::MAX_FILE_BYTES + 1;
            }
            _ => unreachable!(),
        }
        preparation.identity = preparation_identity(&preparation).unwrap();
        fs::write(&path, encode(&preparation).unwrap()).unwrap();
        assert!(recover_bundle(root.path()).is_err(), "{mutation}");
        baseline(root.path());
        assert!(path.is_file());
    }
}

#[test]
fn archive_sync_error_reports_the_actual_retained_location() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    let root = fixture();
    let error = plan(root.path())
        .apply_with(&mut fail(Point::ArchiveSync))
        .unwrap_err();
    let ApplyError::Io { retained, .. } = error else {
        panic!("{error:?}");
    };
    assert!(retained.join("entries/0000/before").is_file());
    intended(root.path());
    assert!(recover_bundle(root.path()).unwrap().is_none());
}

#[test]
fn review_user_deletion_after_installation_is_preserved() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for recovering in [false, true] {
        let root = fixture();
        let result = plan(root.path()).apply_with(&mut |seen| {
            if seen == point(Some(0), Phase::Installed) {
                if recovering {
                    return Err(io::Error::other("interrupt after install"));
                }
                fs::remove_file(root.path().join("a.bzl"))?;
                fs::write(root.path().join("child/c.bzl"), "user")?;
            }
            Ok(())
        });
        let receipt = if recovering {
            assert!(result.is_err());
            fs::remove_file(root.path().join("a.bzl")).unwrap();
            recover_bundle(root.path()).unwrap().unwrap()
        } else {
            result.unwrap()
        };
        assert_eq!(receipt.outcome, BatchOutcome::Conflict);
        assert!(
            !root.path().join("a.bzl").exists(),
            "an installed output's deletion must win"
        );
        assert_eq!(
            receipt.outputs[0].disposition,
            OutputDisposition::Conflict(ConflictReason::MissingDestination)
        );
        assert_eq!(
            fs::read_to_string(receipt.retained.join("entries/0000/before")).unwrap(),
            "old-a"
        );
    }
}

#[test]
fn review_unpublished_preparation_is_recoverable_without_baseline_claims() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for record in [
        RecordPoint::Created,
        RecordPoint::Written,
        RecordPoint::Synced,
    ] {
        let root = fixture();
        assert!(
            plan(root.path())
                .apply_with(&mut fail(point(None, Phase::PreparationRecord(record))))
                .is_err()
        );
        let receipt = recover_bundle(root.path()).unwrap().unwrap();
        assert_eq!(receipt.outcome, BatchOutcome::Unprepared);
        assert!(
            receipt.outputs.is_empty(),
            "unpublished intent cannot verify baselines"
        );
        baseline(root.path());
        let _receipt = plan(root.path()).apply().unwrap();
    }
}

#[test]
fn review_recovery_sync_failure_keeps_installation_and_restore_pending() {
    let _spawn_guard = PROCESS_SPAWN.read().unwrap();
    for restoring in [false, true] {
        let root = fixture();
        assert!(
            plan(root.path())
                .apply_with(&mut |seen| {
                    if restoring && seen == point(Some(0), Phase::BaselineChecked) {
                        fs::write(root.path().join("a.bzl"), "saved")?;
                    }
                    fail(point(
                        Some(0),
                        if restoring {
                            Phase::RestoreSync
                        } else {
                            Phase::InstallSync
                        },
                    ))(seen)
                })
                .is_err()
        );
        assert!(
            recover_with(root.path(), &mut fail(point(Some(0), Phase::ReconcileSync))).is_err()
        );
        assert!(root.path().join(".rebaze-apply/active").is_dir());
        let receipt = recover_bundle(root.path()).unwrap().unwrap();
        if restoring {
            assert_eq!(receipt.outcome, BatchOutcome::Conflict);
            assert_eq!(
                fs::read_to_string(root.path().join("a.bzl")).unwrap(),
                "saved"
            );
        } else {
            assert_eq!(receipt.outcome, BatchOutcome::Applied);
            intended(root.path());
        }
    }
}

#[test]
fn real_process_exit_and_fresh_process_recovery_cover_batch_transitions() {
    let _spawn_guard = PROCESS_SPAWN.write().unwrap();
    for selected in [
        "preparation",
        "stage",
        "stage-directory",
        "intent-record",
        "intent",
        "moved",
        "installed",
        "install-sync",
        "done",
        "restore",
        "restore-intent",
        "stop",
        "archive",
    ] {
        let root = fixture();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "apply::batch::tests::batch_process_helper",
            ])
            .env("REBAZE_BATCH_TEST_ROOT", root.path())
            .env("REBAZE_BATCH_TEST_POINT", selected)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(81), "{selected}");
        if selected == "install-sync" {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "apply::batch::tests::batch_process_helper",
                ])
                .env("REBAZE_BATCH_TEST_ROOT", root.path())
                .env("REBAZE_BATCH_TEST_POINT", "interrupt-recovery")
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(82));
            assert!(root.path().join(".rebaze-apply/active").is_dir());
        }
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "apply::batch::tests::batch_process_helper",
            ])
            .env("REBAZE_BATCH_TEST_ROOT", root.path())
            .env("REBAZE_BATCH_TEST_POINT", format!("recover-{selected}"))
            .status()
            .unwrap();
        assert!(status.success(), "{selected}");
        if matches!(
            selected,
            "preparation" | "stage" | "stage-directory" | "intent-record"
        ) {
            baseline(root.path());
        } else if matches!(selected, "restore" | "restore-intent" | "stop") {
            assert_eq!(
                fs::read_to_string(root.path().join("a.bzl")).unwrap(),
                "saved"
            );
        } else {
            intended(root.path());
        }
    }
}

#[test]
#[ignore = "subprocess helper explicitly run by real_process_exit_and_fresh_process_recovery_cover_batch_transitions"]
fn batch_process_helper() {
    let root = PathBuf::from(std::env::var_os("REBAZE_BATCH_TEST_ROOT").unwrap());
    let selected = std::env::var("REBAZE_BATCH_TEST_POINT").unwrap();
    if selected == "interrupt-recovery" {
        let _receipt = recover_with(&root, &mut |seen| {
            if seen == point(Some(0), Phase::ReconcileSync) {
                std::process::exit(82);
            }
            Ok(())
        })
        .unwrap();
        panic!("recovery interruption point was not reached");
    }
    if selected.starts_with("recover-") {
        let receipt = recover_bundle(&root).unwrap();
        if selected == "recover-archive" {
            assert!(receipt.is_none());
        } else {
            let expected = if selected == "recover-preparation" {
                BatchOutcome::Unprepared
            } else if matches!(
                selected.as_str(),
                "recover-stage" | "recover-stage-directory" | "recover-intent-record"
            ) {
                BatchOutcome::BaselineVerified
            } else if matches!(
                selected.as_str(),
                "recover-restore" | "recover-restore-intent" | "recover-stop"
            ) {
                BatchOutcome::Conflict
            } else {
                BatchOutcome::Applied
            };
            assert_eq!(receipt.unwrap().outcome, expected);
        }
        assert!(recover_bundle(&root).unwrap().is_none());
        return;
    }
    let _receipt = plan(&root)
        .apply_with(&mut |seen| {
            if matches!(selected.as_str(), "restore" | "restore-intent" | "stop")
                && seen == point(Some(0), Phase::BaselineChecked)
            {
                fs::write(root.join("a.bzl"), "saved")?;
            }
            let chosen = match selected.as_str() {
                "preparation" => point(None, Phase::PreparationRecord(RecordPoint::Written)),
                "stage" => point(Some(1), Phase::StageSync),
                "stage-directory" => point(Some(1), Phase::StageDirectorySync),
                "intent-record" => point(None, Phase::IntentRecord(RecordPoint::Written)),
                "intent" => point(None, Phase::IntentDurable),
                "moved" => point(Some(0), Phase::Moved),
                "installed" => point(Some(0), Phase::Installed),
                "install-sync" => point(Some(0), Phase::InstallSync),
                "done" => point(Some(0), Phase::DoneRecord(RecordPoint::Written)),
                "restore" => point(Some(0), Phase::RestoreSync),
                "restore-intent" => point(Some(0), Phase::RestoreRecord(RecordPoint::Published)),
                "stop" => point(None, Phase::StopRecord(RecordPoint::Synced)),
                "archive" => Point::ArchiveSync,
                _ => panic!("unknown process case"),
            };
            if seen == chosen {
                std::process::exit(81);
            }
            Ok(())
        })
        .unwrap();
    panic!("selected process point was not reached");
}
