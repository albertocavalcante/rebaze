//! Parse a list of CMake files and report failures.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let list_path = arg_value(&args, "--list").map_or_else(default_list_path, PathBuf::from);
    let limit = arg_value(&args, "--limit")
        .and_then(|val| val.parse::<usize>().ok());
    let allow_fail = args.iter().any(|arg| arg == "--allow-fail");
    let max_errors = arg_value(&args, "--max-errors")
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(20);
    let max_empty = arg_value(&args, "--max-empty")
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(10);
    let max_success = arg_value(&args, "--max-success")
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(10);

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

    let mut read_errors = Vec::new();
    let mut parse_errors = Vec::new();
    let mut empty_files = Vec::new();
    let mut non_empty_count = 0usize;
    let mut sample_success = Vec::new();

    for path in files {
        let src = match fs::read_to_string(path) {
            Ok(src) => src,
            Err(err) => {
                read_errors.push(format!("{path}: read error: {err}"));
                continue;
            }
        };

        match rebaze_cmake::parse_source(&src) {
            Ok(file) => {
                if file.commands.is_empty() {
                    empty_files.push(path.to_string());
                } else {
                    non_empty_count += 1;
                    if sample_success.len() < max_success {
                        sample_success.push(path.to_string());
                    }
                }
            }
            Err(err) => parse_errors.push(format!("{path}: {err}")),
        }
    }

    let total = read_errors.len() + parse_errors.len() + empty_files.len() + non_empty_count;
    let read_ok = total.saturating_sub(read_errors.len());
    let parsed_ok = read_ok.saturating_sub(parse_errors.len());
    println!("Total files: {total}");
    println!("Read ok: {read_ok}, read errors: {}", read_errors.len());
    println!(
        "Parsed ok: {parsed_ok} (non-empty: {non_empty_count}, empty: {})",
        empty_files.len()
    );
    println!("Parse errors: {}", parse_errors.len());

    for err in read_errors
        .iter()
        .chain(parse_errors.iter())
        .take(max_errors)
    {
        println!("  - {err}");
    }

    let total_failures = read_errors.len() + parse_errors.len();
    if total_failures > max_errors {
        println!("  ... {} more", total_failures - max_errors);
    }

    if !empty_files.is_empty() {
        println!("Empty AST (no commands): {}", empty_files.len());
        for path in empty_files.iter().take(max_empty) {
            println!("  - {path}");
        }
        if empty_files.len() > max_empty {
            println!("  ... {} more", empty_files.len() - max_empty);
        }
    }

    if !sample_success.is_empty() {
        println!("Parsed with commands (sample):");
        for path in &sample_success {
            println!("  - {path}");
        }
        if non_empty_count > sample_success.len() {
            println!("  ... {} more", non_empty_count - sample_success.len());
        }
    }

    if parse_errors.is_empty() && read_errors.is_empty() || allow_fail {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Parser failures: {}",
            read_errors.len() + parse_errors.len()
        ))
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
