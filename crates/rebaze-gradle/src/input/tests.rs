use super::*;
use std::fs;

fn project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("settings.gradle"),
        "rootProject.name = 'example'",
    )
    .unwrap();
    root
}

#[test]
fn absent_sources_and_build_files_retain_defaults_but_ambiguity_is_an_error() {
    let root = project();
    let parsed = read_project(root.path(), ReadLimits::default()).unwrap();
    assert_eq!(parsed.modules.len(), 1);
    assert_eq!(parsed.modules[0].sources, [] as [String; 0]);
    assert_eq!(parsed.maven_lock, None);
    fs::write(root.path().join("build.gradle"), "").unwrap();
    fs::write(root.path().join("build.gradle.kts"), "").unwrap();
    assert!(
        read_project(root.path(), ReadLimits::default())
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
}

#[test]
fn nested_modules_keep_relative_paths_and_scan_complete_java_files() {
    let root = project();
    fs::write(root.path().join("settings.gradle"), "include(':group:app')").unwrap();
    let tests = root.path().join("group/app/src/test/java/example");
    fs::create_dir_all(&tests).unwrap();
    let java = format!(
        "/*{}*/ package example; import org.junit.Test; public class Behavior {{ @Test public void value() {{}} }}",
        "x".repeat(16 * 1024)
    );
    fs::write(tests.join("Behavior.java"), java).unwrap();
    let parsed = read_project(root.path(), ReadLimits::default()).unwrap();
    assert_eq!(parsed.modules.len(), 3);
    let module = parsed
        .modules
        .iter()
        .find(|module| module.path == ":group:app")
        .unwrap();
    assert_eq!(module.test_sources, ["src/test/java/example/Behavior.java"]);
    assert_eq!(module.test_classes, ["example.Behavior"]);
}

#[test]
fn file_and_aggregate_budgets_include_settings_build_java_and_lock() {
    let root = project();
    let settings = fs::read_to_string(root.path().join("settings.gradle")).unwrap();
    fs::write(root.path().join("build.gradle"), "").unwrap();
    fs::write(root.path().join("maven_install.json"), "{}").unwrap();
    let exact = ReadLimits::new(settings.len(), settings.len() + 2, 3).unwrap();
    assert!(read_project(root.path(), exact).is_ok());
    let bytes = ReadLimits::new(settings.len(), settings.len() + 1, 3).unwrap();
    assert!(
        read_project(root.path(), bytes)
            .unwrap_err()
            .to_string()
            .contains("aggregate bytes")
    );
    let files = ReadLimits::new(settings.len(), settings.len() + 2, 2).unwrap();
    assert!(
        read_project(root.path(), files)
            .unwrap_err()
            .to_string()
            .contains("file attempts")
    );
}

#[test]
fn module_expansion_counts_root_implicit_ancestors_and_duplicates() {
    assert_eq!(
        module_paths(vec![vec!["a"; 1023].join(":")]).unwrap().len(),
        1024
    );
    assert!(
        module_paths(vec![vec!["a"; 1024].join(":")])
            .unwrap_err()
            .to_string()
            .contains("module count budget")
    );
    let siblings = (0..1023).map(|index| format!(":p{index}")).collect();
    assert_eq!(module_paths(siblings).unwrap().len(), 1024);
    let siblings = (0..1024).map(|index| format!(":p{index}")).collect();
    assert!(
        module_paths(siblings)
            .unwrap_err()
            .to_string()
            .contains("module count budget")
    );
    assert_eq!(
        module_paths(vec![":a:b:c".into(), ":a:b".into()])
            .unwrap()
            .len(),
        4
    );
    assert!(module_paths(vec![":a".into(), ":a".into()]).is_err());
    assert!(module_paths(vec![":".into()]).is_err());
    assert!(project_path(&"x".repeat(4095)).is_ok());
    assert!(project_path(&"x".repeat(4096)).is_err());
    assert!(project_path(&format!(":{}", "x".repeat(4095))).is_ok());
    let error = project_path(&"x".repeat(1024 * 1024)).unwrap_err();
    assert!(error.to_string().len() < 256);
}

#[test]
fn filtered_files_still_exhaust_the_directory_entry_budget() {
    let root = project();
    let source = root.path().join("src/main/java");
    fs::create_dir_all(&source).unwrap();
    for name in ["a.bin", "b.bin", "c.bin"] {
        fs::write(source.join(name), "").unwrap();
    }
    let limits = ReadLimits::default().with_directory_entries(2).unwrap();
    assert!(
        read_project(root.path(), limits)
            .unwrap_err()
            .to_string()
            .contains("directory entries")
    );
    let limits = ReadLimits::default().with_directory_entries(3).unwrap();
    assert_eq!(
        read_project(root.path(), limits).unwrap().modules[0].sources,
        [] as [String; 0]
    );
}

#[cfg(unix)]
#[test]
fn links_and_special_roots_fail() {
    use std::os::unix::fs::symlink;
    let root = project();
    let source = root.path().join("src/main/java");
    fs::create_dir_all(&source).unwrap();
    symlink("missing", source.join("ignored.link")).unwrap();
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
    fs::remove_file(source.join("ignored.link")).unwrap();
    fs::write(root.path().join("src/main/resources"), "not a directory").unwrap();
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
}

#[cfg(unix)]
#[test]
fn module_and_source_ancestor_links_are_rejected_and_lock_links_are_not_absent() {
    use std::os::unix::fs::symlink;
    let root = project();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("real")).unwrap();
    symlink("real", root.path().join("app")).unwrap();
    fs::write(root.path().join("settings.gradle"), "include(':app')").unwrap();
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
    fs::remove_file(root.path().join("app")).unwrap();
    symlink(outside.path(), root.path().join("app")).unwrap();
    fs::write(root.path().join("settings.gradle"), "include(':app')").unwrap();
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
    fs::write(root.path().join("settings.gradle"), "").unwrap();
    symlink("missing", root.path().join("src")).unwrap();
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
    fs::remove_file(root.path().join("src")).unwrap();
    symlink("missing", root.path().join("maven_install.json")).unwrap();
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
}

#[cfg(unix)]
#[test]
fn filename_filtering_precedes_utf8_conversion_but_rejects_selected_unsupported_paths() {
    use std::os::unix::ffi::OsStringExt;
    let ignored = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff, b'.', b'b']));
    assert_eq!(source_path(Path::new(""), &ignored, true).unwrap(), None);
    assert!(source_path(Path::new(""), &ignored, false).is_err());
    let java = PathBuf::from(std::ffi::OsString::from_vec(vec![
        0xff, b'.', b'j', b'a', b'v', b'a',
    ]));
    assert!(source_path(Path::new(""), &java, true).is_err());
    assert!(
        source_path(
            Path::new(""),
            Path::new("src/main/resources/literal\\backslash"),
            false
        )
        .is_err()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_filesystem_discovery_counts_but_ignores_non_utf8_non_java_files() {
    use std::os::unix::ffi::OsStringExt;
    let root = project();
    let directory = root.path().join("src/main/java");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(std::ffi::OsString::from_vec(vec![0xff, b'.', b'b'])),
        "",
    )
    .unwrap();
    assert_eq!(
        read_project(root.path(), ReadLimits::default())
            .unwrap()
            .modules[0]
            .sources,
        [] as [String; 0]
    );
    let limits = ReadLimits::default().with_directory_entries(1).unwrap();
    fs::write(directory.join("another.bin"), "").unwrap();
    assert!(read_project(root.path(), limits).is_err());
}

#[cfg(unix)]
#[test]
fn unreadable_source_directories_are_not_treated_as_absent() {
    use std::os::unix::fs::PermissionsExt;
    let root = project();
    let directory = root.path().join("src/main/java");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("Value.java"), "class Value {}").unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o0)).unwrap();
    let denied = fs::read_dir(&directory).is_err();
    let result = read_project(root.path(), ReadLimits::default());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    if denied {
        assert!(result.is_err());
    } else {
        assert!(result.is_ok());
    }
}

#[cfg(unix)]
#[test]
fn source_enumeration_uses_the_reader_after_nominal_root_replacement() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    fs::create_dir_all(root.join("src/main/java")).unwrap();
    fs::write(root.join("src/main/java/Original.java"), "").unwrap();
    let mut reader = RootReader::open(&root, ReadLimits::default(), SymlinkPolicy::Reject).unwrap();
    fs::rename(&root, parent.path().join("moved")).unwrap();
    fs::create_dir_all(root.join("src/main/java")).unwrap();
    fs::write(root.join("src/main/java/Replacement.java"), "").unwrap();
    assert_eq!(
        files(&mut reader, Path::new(""), "src/main/java", true).unwrap(),
        ["src/main/java/Original.java"]
    );
}

#[cfg(unix)]
#[test]
fn special_source_and_lock_files_fail_in_a_bounded_fresh_process() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "input::tests::fifo_child",
            "--ignored",
            "--nocapture",
        ])
        .env("REBAZE_GRADLE_FIFO_CHILD", "1")
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
            panic!("Gradle special-file capture blocked");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
#[ignore = "watchdog parent runs this fresh-process helper"]
fn fifo_child() {
    assert_eq!(std::env::var("REBAZE_GRADLE_FIFO_CHILD").unwrap(), "1");
    let root = project();
    let source = root.path().join("src/main/java");
    fs::create_dir_all(&source).unwrap();
    let fifo = source.join("ignored.bin");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
    fs::remove_file(&fifo).unwrap();
    let fifo = root.path().join("maven_install.json");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(read_project(root.path(), ReadLimits::default()).is_err());
}
