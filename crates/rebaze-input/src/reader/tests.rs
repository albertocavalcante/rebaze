use super::*;
use std::fs;

fn reader(root: &Path, file: usize, total: usize, files: usize) -> RootReader {
    RootReader::open(
        root,
        ReadLimits::new(file, total, files).unwrap(),
        SymlinkPolicy::Reject,
    )
    .unwrap()
}

#[test]
fn validates_limits_and_paths_before_io() {
    for value in [0, usize::MAX] {
        assert!(ReadLimits::new(value, 512 * 1024 * 1024, 1).is_err());
        assert!(ReadLimits::new(1, value, 1).is_err());
        assert!(ReadLimits::new(1, 1, value).is_err());
        assert!(ReadLimits::default().with_traversal(value, 1).is_err());
        assert!(ReadLimits::default().with_traversal(1, value).is_err());
    }
    assert!(ReadLimits::new(2, 1, 1).is_err());
    let root = tempfile::tempdir().unwrap();
    let mut input = reader(root.path(), 4, 8, 2);
    assert_eq!(
        input.kind(Path::new("")).unwrap(),
        Some(EntryKind::Directory)
    );
    assert_eq!(
        input.kind(Path::new(".")).unwrap(),
        Some(EntryKind::Directory)
    );
    for path in ["", ".", "../escape", "a/../../escape", "/absolute"] {
        assert!(matches!(
            input.read_text(Path::new(path)),
            Err(ReadError::InvalidPath(_))
        ));
    }
    assert_eq!(input.usage().file_attempts(), 0);
}

#[test]
fn exact_bytes_utf8_and_failed_reads_share_one_budget() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a"), "éé").unwrap();
    fs::write(root.path().join("invalid"), [0xff; 4]).unwrap();
    fs::write(root.path().join("large"), [b'x'; 5]).unwrap();
    let mut input = reader(root.path(), 4, 8, 8);
    assert_eq!(input.read_text(Path::new("a")).unwrap(), "éé");
    assert!(matches!(
        input.read_text(Path::new("large")),
        Err(ReadError::Limit {
            kind: LimitKind::FileBytes,
            ..
        })
    ));
    assert_eq!(input.usage().bytes(), 4);
    assert!(
        matches!(input.read_text(Path::new("invalid")), Err(ReadError::Utf8 { source, .. }) if source.valid_up_to() == 0)
    );
    assert_eq!(input.usage().bytes(), 8);
    assert!(matches!(
        input.read_text(Path::new("a")),
        Err(ReadError::Limit {
            kind: LimitKind::AggregateBytes,
            ..
        })
    ));
    assert_eq!(input.usage().bytes(), 9);
    assert!(input.read_text(Path::new("a")).is_err());
    assert_eq!(
        input.usage().bytes(),
        9,
        "an exhausted budget must not consume further proof bytes"
    );
}

#[test]
fn failed_probes_attempts_depth_and_operation_limits_are_charged() {
    let root = tempfile::tempdir().unwrap();
    let mut input = reader(root.path(), 4, 8, 2);
    for _ in 0..2 {
        assert!(input.read_text(Path::new("missing")).is_err());
    }
    assert!(matches!(
        input.read_text(Path::new("missing")),
        Err(ReadError::Limit {
            kind: LimitKind::FileAttempts,
            observed: 3,
            ..
        })
    ));
    let limits = ReadLimits::new(4, 8, 2)
        .unwrap()
        .with_traversal(3, 1)
        .unwrap();
    let mut input = RootReader::open(root.path(), limits, SymlinkPolicy::Reject).unwrap();
    assert!(matches!(
        input.kind(Path::new("a/b")),
        Err(ReadError::Limit {
            kind: LimitKind::PathDepth,
            ..
        })
    ));
    assert_eq!(input.kind(Path::new("missing")).unwrap(), None);
    assert_eq!(input.kind(Path::new("missing")).unwrap(), None);
    assert!(matches!(
        input.kind(Path::new("missing")),
        Err(ReadError::Limit {
            kind: LimitKind::Operations,
            ..
        })
    ));
}

#[test]
fn optional_files_require_existing_parents_and_preserve_errors() {
    let root = tempfile::tempdir().unwrap();
    for policy in [SymlinkPolicy::Reject, SymlinkPolicy::WithinRoot] {
        let mut input = RootReader::open(root.path(), ReadLimits::default(), policy).unwrap();
        assert_eq!(input.read_optional_text(Path::new("absent")).unwrap(), None);
        assert!(input.read_optional_text(Path::new("missing/file")).is_err());
        fs::write(root.path().join("invalid"), [0xff]).unwrap();
        assert!(matches!(
            input.read_optional_text(Path::new("invalid")),
            Err(ReadError::Utf8 { .. })
        ));
        fs::remove_file(root.path().join("invalid")).unwrap();
    }
}

#[test]
fn actual_read_counts_bound_growing_streams_and_interrupted_reads() {
    struct Interrupted;
    impl Read for Interrupted {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::Interrupted.into())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut input = reader(root.path(), 4, 8, 2);
    let mut stream = io::repeat(b'x');
    assert!(matches!(
        input.counted(Path::new("growing"), &mut stream),
        Err(ReadError::Limit {
            kind: LimitKind::FileBytes,
            observed: 5,
            ..
        })
    ));
    assert_eq!(input.usage().bytes(), 5);
    input.limits = input.limits.with_traversal(3, 1).unwrap();
    assert!(matches!(
        input.counted(Path::new("interruptions"), &mut Interrupted),
        Err(ReadError::Limit {
            kind: LimitKind::Operations,
            ..
        })
    ));
    assert_eq!(input.usage().operations(), 3);
}

#[test]
#[cfg(unix)]
fn opened_root_survives_rename_without_reading_a_replacement_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), "old").unwrap();
    let mut input = reader(&root, 4, 8, 2);
    fs::rename(&root, parent.path().join("moved")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), "new").unwrap();
    assert_eq!(input.read_text(Path::new("file")).unwrap(), "old");
}

#[test]
#[cfg(windows)]
fn live_windows_root_rejects_rename_and_reads_the_original() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), "old").unwrap();
    let mut input = reader(&root, 4, 8, 2);
    assert!(fs::rename(&root, parent.path().join("moved")).is_err());
    assert_eq!(input.read_text(Path::new("file")).unwrap(), "old");
}

#[test]
fn rejected_paths_are_charged_without_copying_oversized_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let limits = ReadLimits::default().with_traversal(2, 1).unwrap();
    let mut input = RootReader::open(root.path(), limits, SymlinkPolicy::Reject).unwrap();
    assert!(matches!(
        input.read_text(Path::new("../escape")),
        Err(ReadError::InvalidPath(_))
    ));
    let oversized = "x".repeat(1024 * 1024);
    let error = input.read_text(Path::new(&oversized)).unwrap_err();
    assert!(matches!(error, ReadError::PathTooLong { bytes: 1_048_576 }));
    assert!(format!("{error:?}").len() < 128);
    let error = input.kind(Path::new(&oversized)).unwrap_err();
    assert!(matches!(
        error,
        ReadError::Limit {
            kind: LimitKind::Operations,
            ..
        }
    ));
    assert!(format!("{error:?}").len() < 256);
    assert_eq!(input.usage().operations(), 2);
    assert_eq!(input.usage().file_attempts(), 0);
}

#[test]
fn changes_after_open_are_rejected_and_bytes_stay_charged() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file");
    fs::write(&path, "old").unwrap();
    let mut input = reader(root.path(), 4, 8, 2);
    let result = input.read_with(Path::new("file"), &mut |point| {
        if point == Point::AfterOpen {
            fs::write(&path, "grow")?;
        }
        Ok(())
    });
    assert!(matches!(result, Err(ReadError::Changed(_))));
    assert_eq!(input.usage().bytes(), 4);
}

#[test]
fn sparse_oversize_and_nonregular_inputs_fail_before_reading() {
    let root = tempfile::tempdir().unwrap();
    fs::File::create(root.path().join("sparse"))
        .unwrap()
        .set_len(1 << 32)
        .unwrap();
    fs::create_dir(root.path().join("directory")).unwrap();
    let mut input = reader(root.path(), 4, 8, 2);
    assert!(matches!(
        input.read_text(Path::new("sparse")),
        Err(ReadError::Limit {
            kind: LimitKind::FileBytes,
            observed: 4_294_967_296,
            ..
        })
    ));
    assert!(matches!(
        input.read_text(Path::new("directory")),
        Err(ReadError::NotRegular(_))
    ));
    assert_eq!(input.usage().bytes(), 0);
}

#[cfg(unix)]
#[test]
fn symlink_profiles_cover_ancestors_internal_parents_escape_and_cycles() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("real")).unwrap();
    fs::create_dir(root.path().join("other")).unwrap();
    fs::write(root.path().join("real/file"), "safe").unwrap();
    fs::write(outside.path().join("file"), "evil").unwrap();
    symlink("real", root.path().join("alias")).unwrap();
    symlink("../real/file", root.path().join("other/internal")).unwrap();
    symlink(root.path().join("real/file"), root.path().join("absolute")).unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    symlink("missing", root.path().join("broken")).unwrap();
    symlink("loop", root.path().join("loop")).unwrap();
    let mut reject = reader(root.path(), 8, 64, 16);
    for path in [
        "alias/file",
        "other/internal",
        "absolute",
        "escape/file",
        "broken",
        "loop",
    ] {
        assert!(
            reject.read_text(Path::new(path)).is_err(),
            "followed {path}"
        );
    }
    assert_eq!(
        reject.kind(Path::new("broken")).unwrap(),
        Some(EntryKind::Symlink)
    );
    let mut within = RootReader::open(
        root.path(),
        ReadLimits::new(8, 64, 16).unwrap(),
        SymlinkPolicy::WithinRoot,
    )
    .unwrap();
    assert!(within.read_optional_text(Path::new("broken/file")).is_err());
    assert!(within.read_optional_text(Path::new("broken")).is_err());
    for path in ["alias/file", "other/internal"] {
        assert_eq!(within.read_text(Path::new(path)).unwrap(), "safe");
    }
    assert_eq!(
        within.canonical_dir(Path::new("alias")).unwrap(),
        Path::new("real")
    );
    for path in ["absolute", "escape/file", "broken", "loop"] {
        assert!(
            within.read_text(Path::new(path)).is_err(),
            "followed {path}"
        );
    }
}

#[cfg(unix)]
#[test]
fn ancestor_swap_between_metadata_and_open_cannot_escape() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("parent")).unwrap();
    fs::write(root.path().join("parent/file"), "safe").unwrap();
    fs::write(outside.path().join("file"), "evil").unwrap();
    for policy in [SymlinkPolicy::Reject, SymlinkPolicy::WithinRoot] {
        let mut input =
            RootReader::open(root.path(), ReadLimits::new(8, 8, 1).unwrap(), policy).unwrap();
        let result = input.read_with(Path::new("parent/file"), &mut |point| {
            if point == Point::AfterMetadata {
                fs::rename(root.path().join("parent"), root.path().join("saved"))?;
                symlink(outside.path(), root.path().join("parent"))?;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(input.usage().bytes(), 0);
        fs::remove_file(root.path().join("parent")).unwrap();
        fs::rename(root.path().join("saved"), root.path().join("parent")).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn replacement_fifo_cannot_block_after_regular_file_preflight() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reader::tests::fifo_child",
            "--ignored",
            "--nocapture",
        ])
        .env("REBAZE_INPUT_FIFO_CHILD", "1")
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("FIFO open blocked");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
#[ignore = "watchdog parent runs this fresh-process helper"]
fn fifo_child() {
    assert_eq!(std::env::var("REBAZE_INPUT_FIFO_CHILD").unwrap(), "1");
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file"), "old").unwrap();
    let mut input = reader(root.path(), 4, 8, 1);
    let result = input.read_with(Path::new("file"), &mut |point| {
        if point == Point::AfterMetadata {
            fs::remove_file(root.path().join("file"))?;
            let status = std::process::Command::new("mkfifo")
                .arg(root.path().join("file"))
                .status()?;
            if !status.success() {
                return Err(io::Error::other("mkfifo failed"));
            }
        }
        Ok(())
    });
    assert!(matches!(result, Err(ReadError::NotRegular(_))));
}
