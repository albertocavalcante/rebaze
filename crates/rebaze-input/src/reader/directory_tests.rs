use super::*;
use std::fs;

fn reader(path: &Path, entries: usize, operations: usize, depth: usize) -> RootReader {
    let limits = ReadLimits::default()
        .with_directory_entries(entries)
        .unwrap()
        .with_traversal(operations, depth)
        .unwrap();
    RootReader::open(path, limits, SymlinkPolicy::Reject).unwrap()
}

#[test]
fn directory_budget_accepts_exact_width_and_charges_the_proof_entry() {
    assert!(ReadLimits::default().with_directory_entries(65_536).is_ok());
    for value in [0, 65_537, usize::MAX] {
        assert!(ReadLimits::default().with_directory_entries(value).is_err());
    }
    let root = tempfile::tempdir().unwrap();
    for name in ["c", "a", "b"] {
        fs::write(root.path().join(name), "").unwrap();
    }
    let mut exact = reader(root.path(), 3, 100, 4);
    let entries = exact.directory_entries(Path::new("")).unwrap();
    assert_eq!(
        entries.iter().map(DirectoryEntry::name).collect::<Vec<_>>(),
        [OsStr::new("a"), OsStr::new("b"), OsStr::new("c")]
    );
    assert_eq!(exact.usage().directory_entries(), 3);
    assert_eq!(exact.usage().bytes(), 0);
    assert_eq!(exact.usage().file_attempts(), 0);
    let mut small = reader(root.path(), 2, 100, 4);
    assert!(matches!(
        small.directory_entries(Path::new("")),
        Err(ReadError::Limit {
            kind: LimitKind::DirectoryEntries,
            observed: 3,
            ..
        })
    ));
    assert_eq!(small.usage().directory_entries(), 3);
    assert!(small.directory_entries(Path::new("")).is_err());
    assert_eq!(small.usage().directory_entries(), 3);
}

#[test]
fn directory_iterator_errors_are_charged_before_the_error_is_returned() {
    let root = tempfile::tempdir().unwrap();
    let mut input = reader(root.path(), 4, 100, 4);
    let entries = std::iter::once(Err(io::Error::from(io::ErrorKind::PermissionDenied)));
    assert!(
        matches!(input.collect_directory(Path::new(""), entries), Err(ReadError::Io {source, ..}) if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert_eq!(input.usage().directory_entries(), 1);
    assert_eq!(input.usage().operations(), 1);
}

#[test]
fn default_entry_limit_accepts_8192_and_rejects_8193() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..8192 {
        fs::write(root.path().join(format!("file-{index:04}")), "").unwrap();
    }
    let mut exact = reader(root.path(), 8192, 65_536, 4);
    assert_eq!(exact.directory_entries(Path::new("")).unwrap().len(), 8192);
    fs::write(root.path().join("proof-entry"), "").unwrap();
    let mut above = reader(root.path(), 8192, 65_536, 4);
    assert!(matches!(
        above.directory_entries(Path::new("")),
        Err(ReadError::Limit {
            kind: LimitKind::DirectoryEntries,
            observed: 8193,
            ..
        })
    ));
}

#[test]
fn all_entries_share_one_budget_across_repeated_directory_scans() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    fs::write(root.path().join("ignored.bin"), "").unwrap();
    fs::write(root.path().join("child/file"), "").unwrap();
    let mut input = reader(root.path(), 3, 100, 4);
    assert_eq!(input.directory_entries(Path::new("")).unwrap().len(), 2);
    assert_eq!(
        input.directory_entries(Path::new("child")).unwrap().len(),
        1
    );
    assert!(matches!(
        input.directory_entries(Path::new("child")),
        Err(ReadError::Limit {
            kind: LimitKind::DirectoryEntries,
            observed: 4,
            ..
        })
    ));
}

#[test]
fn operations_and_depth_exhaust_independently_of_entry_counts() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    fs::write(root.path().join("child/file"), "").unwrap();
    let mut operations = reader(root.path(), 100, 2, 4);
    assert!(matches!(
        operations.directory_entries(Path::new("")),
        Err(ReadError::Limit {
            kind: LimitKind::Operations,
            ..
        })
    ));
    assert_eq!(operations.usage().directory_entries(), 1);
    let mut depth = reader(root.path(), 100, 100, 1);
    assert!(matches!(
        depth.directory_entries(Path::new("child")),
        Err(ReadError::Limit {
            kind: LimitKind::PathDepth,
            observed: 2,
            ..
        })
    ));
}

#[test]
fn child_paths_check_bytes_before_joining_and_keep_errors_small() {
    let parent = PathBuf::from("a".repeat(4094));
    assert_eq!(
        child_path(&parent, OsStr::new("b"))
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
            .len(),
        4096
    );
    assert!(matches!(
        child_path(&parent, OsStr::new("é")),
        Err(ReadError::PathTooLong { bytes: 4097 })
    ));
    let trailing = PathBuf::from(format!("{}/", "a".repeat(4094)));
    assert_eq!(
        child_path(&trailing, OsStr::new("b"))
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
            .len(),
        4096
    );
    assert_eq!(
        child_path(Path::new(""), OsStr::new("x")).unwrap(),
        Path::new("x")
    );
    let name = "x".repeat(1024 * 1024);
    let error = child_path(Path::new(""), OsStr::new(&name)).unwrap_err();
    assert!(format!("{error:?}").len() < 128);
}

#[cfg(unix)]
#[test]
fn links_are_observed_and_replaced_queued_directories_are_not_followed() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    symlink("missing", root.path().join("broken")).unwrap();
    let mut input = reader(root.path(), 10, 100, 4);
    let entries = input.directory_entries(Path::new("")).unwrap();
    assert_eq!(entries[0].kind(), EntryKind::Symlink);
    fs::remove_dir(root.path().join("child")).unwrap();
    symlink(outside.path(), root.path().join("child")).unwrap();
    assert!(input.directory_entries(Path::new("child")).is_err());
    assert_eq!(input.usage().directory_entries(), 2);
}

#[cfg(unix)]
#[test]
fn directory_listing_stays_with_the_opened_root_after_rename() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("original"), "").unwrap();
    let mut input = reader(&root, 4, 100, 4);
    fs::rename(&root, parent.path().join("moved")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("replacement"), "").unwrap();
    let entries = input.directory_entries(Path::new("")).unwrap();
    assert_eq!(entries[0].name(), OsStr::new("original"));
    assert_eq!(entries.len(), 1);
}
