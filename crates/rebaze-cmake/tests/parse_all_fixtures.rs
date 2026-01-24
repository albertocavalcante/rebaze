//! Test parsing ALL CMake files in the test fixtures.
//! This helps identify what syntax we don't support yet.

#![allow(clippy::unwrap_used)]

use std::path::Path;

#[test]
fn test_parse_nlohmann_json() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-fixtures/json/CMakeLists.txt");

    if !fixture.exists() {
        eprintln!("Skipping: nlohmann/json fixture not found");
        return;
    }

    let src = std::fs::read_to_string(&fixture).unwrap();
    let result = rebaze_cmake::parse_source(&src);

    match result {
        Ok(file) => {
            println!("Successfully parsed {} commands", file.commands.len());
            for cmd in &file.commands {
                println!("  - {} ({} args)", cmd.name, cmd.arguments.len());
            }
        }
        Err(e) => {
            panic!("Failed to parse nlohmann/json CMakeLists.txt:\n{e}");
        }
    }
}

fn try_parse_file(path: &Path) -> Result<usize, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("Read error: {e}"))?;

    match rebaze_cmake::parse_source(&src) {
        Ok(file) => Ok(file.commands.len()),
        Err(e) => Err(format!("{e}")),
    }
}

#[test]
fn test_parse_all_fixtures() {
    let fixtures_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-fixtures/cmake-examples");

    if !fixtures_dir.exists() {
        eprintln!("Skipping: fixtures not found at {}", fixtures_dir.display());
        return;
    }

    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<(String, String)> = Vec::new();

    // Also test json directory if present
    let json_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-fixtures/json");
    let dirs_to_scan: Vec<&Path> = if json_dir.exists() {
        vec![&fixtures_dir, &json_dir]
    } else {
        vec![&fixtures_dir]
    };

    // Walk all CMakeLists.txt files
    for base_dir in &dirs_to_scan {
        for entry in walkdir::WalkDir::new(base_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() == "CMakeLists.txt")
        {
            let path = entry.path();
            let rel_path = path.strip_prefix(&fixtures_dir).unwrap_or(path);

            match try_parse_file(path) {
                Ok(cmd_count) => {
                    passed += 1;
                    println!("✓ {} ({} commands)", rel_path.display(), cmd_count);
                }
                Err(e) => {
                    failed += 1;
                    let short_err = e.lines().next().unwrap_or(&e).to_string();
                    failures.push((rel_path.display().to_string(), short_err.clone()));
                    println!("✗ {} - {}", rel_path.display(), short_err);
                }
            }
        }
    }

    println!("\n=== Summary ===");
    println!("Passed: {passed}");
    println!("Failed: {failed}");
    println!(
        "Success rate: {:.1}%",
        (passed as f64 / (passed + failed) as f64) * 100.0
    );

    if !failures.is_empty() {
        println!("\n=== Failures ===");
        for (path, err) in &failures {
            println!("  {path}: {err}");
        }
    }

    assert_eq!(failed, 0, "Some CMake files failed to parse");
}
