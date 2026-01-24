//! Parse a list of CMake files and report failures.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let list_path = arg_value(&args, "--list")
        .map(PathBuf::from)
        .unwrap_or_else(default_list_path);
    let limit = arg_value(&args, "--limit")
        .and_then(|val| val.parse::<usize>().ok());
    let allow_fail = args.iter().any(|arg| arg == "--allow-fail");
    let max_errors = arg_value(&args, "--max-errors")
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(20);

    let content = fs::read_to_string(&list_path)
        .map_err(|err| anyhow::anyhow!("Failed to read {}: {err}", list_path.display()))?;
    let mut files: Vec<&str> = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    if let Some(limit) = limit {
        files.truncate(limit);
    }

    let mut ok = 0usize;
    let mut failures = Vec::new();

    for path in files {
        let src = match fs::read_to_string(path) {
            Ok(src) => src,
            Err(err) => {
                failures.push(format!("{path}: read error: {err}"));
                continue;
            }
        };

        match rebaze_cmake::parse_source(&src) {
            Ok(_) => ok += 1,
            Err(err) => failures.push(format!("{path}: {err}")),
        }
    }

    let total = ok + failures.len();
    println!("Parsed: {ok}/{total}");
    println!("Failed: {}", failures.len());

    for err in failures.iter().take(max_errors) {
        println!("  - {err}");
    }

    if failures.len() > max_errors {
        println!("  ... {} more", failures.len() - max_errors);
    }

    if failures.is_empty() || allow_fail {
        Ok(())
    } else {
        Err(anyhow::anyhow!("Parser failures: {}", failures.len()))
    }
}

fn default_list_path() -> PathBuf {
    if let Ok(path) = env::var("REBAZE_CMAKE_CORPUS_FILES") {
        return PathBuf::from(path);
    }
    default_artifacts_dir().join("files.txt")
}

fn default_artifacts_dir() -> PathBuf {
    if let Ok(dir) = env::var("REBAZE_CMAKE_CORPUS_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(current) = env::current_dir() {
        if let Some(root) = find_repo_root(&current) {
            return root.join(".rebaze").join("cmake-corpus");
        }
    }
    if let Ok(dir) = env::var("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("rebaze").join("cmake-corpus");
    }
    env::temp_dir().join("rebaze-cmake-corpus")
}

fn find_repo_root(start: &std::path::Path) -> Option<PathBuf> {
    let markers = [".git", "AGENTS.md", "Cargo.toml", "MODULE.bazel"];
    for candidate in start.ancestors() {
        if markers
            .iter()
            .any(|marker| candidate.join(marker).exists())
        {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == key)
        .and_then(|idx| args.get(idx + 1).cloned())
}
