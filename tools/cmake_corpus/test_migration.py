#!/usr/bin/env -S uv run --script
#
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Test rebaze migration against a curated list of well-known CMake projects.

This script:
1. Reads repos.toml for the list of projects to test
2. Clones each repo (shallow, depth=1) to a temp directory
3. Runs `rebaze migrate` on each (optionally with --dry-run)
4. Validates generated files with `bazel query //...`
5. Optionally runs `bazel build //...` for header-only libraries
6. Collects results and generates a report

Usage:
    ./test_migration.py                    # Test all repos (dry-run only)
    ./test_migration.py --validate         # Generate files + validate with bazel query
    ./test_migration.py --validate --build # Also attempt bazel build (header-only only)
    ./test_migration.py --filter fmt,json  # Test specific repos
    ./test_migration.py --parallel 4       # Run 4 tests in parallel
    ./test_migration.py --keep             # Keep cloned repos for inspection
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field, asdict
from datetime import datetime
from pathlib import Path
from typing import Optional
from enum import Enum


class ValidationLevel(Enum):
    """Level of validation to perform."""
    DRY_RUN = "dry_run"      # Just check rebaze doesn't crash
    QUERY = "query"          # Generate files + bazel query //...
    BUILD = "build"          # Generate files + bazel build //...


@dataclass
class RepoConfig:
    """Configuration for a repository to test."""

    name: str
    url: str
    description: str = ""
    ref: str = ""
    features: list[str] = field(default_factory=list)
    skip: bool = False
    # Header-only libraries are safe to attempt building
    header_only: bool = False


@dataclass
class ValidationResult:
    """Result of validating generated Bazel files."""
    query_success: bool = False
    query_output: str = ""
    query_error: str = ""
    build_success: bool = False
    build_output: str = ""
    build_error: str = ""
    targets_found: int = 0


@dataclass
class TestResult:
    """Result of a migration test."""

    name: str
    url: str
    success: bool
    duration_seconds: float
    exit_code: int
    stdout: str
    stderr: str
    error: str = ""
    files_generated: list[str] = field(default_factory=list)
    skipped: bool = False
    skip_reason: str = ""
    # Validation results
    validation: ValidationResult = field(default_factory=ValidationResult)
    validation_level: str = "dry_run"


def find_rebaze_binary() -> Path | None:
    """Find the rebaze binary."""
    # Check if in PATH
    if shutil.which("rebaze"):
        return Path("rebaze")

    # Check common build locations relative to repo root
    script_dir = Path(__file__).parent
    repo_root = script_dir.parent.parent

    candidates = [
        repo_root / "target" / "release" / "rebaze",
        repo_root / "target" / "debug" / "rebaze",
        repo_root / "bazel-bin" / "crates" / "rebaze-cli" / "rebaze",
    ]

    for candidate in candidates:
        if candidate.exists() and candidate.is_file():
            return candidate

    return None


def find_bazel_binary() -> Path | None:
    """Find the bazel or bazelisk binary."""
    for name in ["bazelisk", "bazel"]:
        path = shutil.which(name)
        if path:
            return Path(path)
    return None


def clone_repo(url: str, dest: Path, ref: str = "") -> tuple[bool, str]:
    """Clone a repository with depth=1."""
    cmd = ["git", "clone", "--depth=1"]
    if ref:
        cmd.extend(["--branch", ref])
    cmd.extend([url, str(dest)])

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=300,  # 5 minute timeout for clone
        )
        if result.returncode != 0:
            return False, result.stderr
        return True, ""
    except subprocess.TimeoutExpired:
        return False, "Clone timed out after 5 minutes"
    except Exception as e:
        return False, str(e)


def run_migration(
    rebaze_bin: Path,
    project_dir: Path,
    dry_run: bool = True,
    timeout: int = 120,
) -> tuple[int, str, str]:
    """Run rebaze migrate on a project."""
    cmd = [str(rebaze_bin), "migrate", "--unsafe-mode"]
    if dry_run:
        cmd.append("--dry-run")
    cmd.append(str(project_dir))

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=project_dir,
        )
        return result.returncode, result.stdout, result.stderr
    except subprocess.TimeoutExpired:
        return -1, "", f"Migration timed out after {timeout} seconds"
    except Exception as e:
        return -1, "", str(e)


def run_bazel_query(
    bazel_bin: Path,
    project_dir: Path,
    timeout: int = 60,
) -> tuple[bool, str, str, int]:
    """Run bazel query //... to validate Starlark syntax."""
    cmd = [str(bazel_bin), "query", "//...", "--output=label"]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=project_dir,
        )
        # Count targets found
        targets = [l for l in result.stdout.strip().split("\n") if l.startswith("//")]
        return result.returncode == 0, result.stdout, result.stderr, len(targets)
    except subprocess.TimeoutExpired:
        return False, "", f"Bazel query timed out after {timeout} seconds", 0
    except Exception as e:
        return False, "", str(e), 0


def run_bazel_build(
    bazel_bin: Path,
    project_dir: Path,
    timeout: int = 300,
) -> tuple[bool, str, str]:
    """Run bazel build //... to verify the project builds."""
    cmd = [str(bazel_bin), "build", "//..."]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=project_dir,
        )
        return result.returncode == 0, result.stdout, result.stderr
    except subprocess.TimeoutExpired:
        return False, "", f"Bazel build timed out after {timeout} seconds"
    except Exception as e:
        return False, "", str(e)


def extract_generated_files(stdout: str) -> list[str]:
    """Extract list of generated files from dry-run output."""
    files = []
    in_file = False
    current_file = ""

    for line in stdout.splitlines():
        if line.startswith("--- ") and line.endswith(" ---"):
            if in_file and current_file:
                files.append(current_file)
            current_file = line[4:-4].strip()
            in_file = True

    if in_file and current_file:
        files.append(current_file)

    return files


def list_generated_files(project_dir: Path) -> list[str]:
    """List Bazel files generated in the project directory."""
    files = []
    bazel_files = [
        "MODULE.bazel",
        "BUILD.bazel",
        "BUILD",
        ".bazelrc",
        ".bazelversion",
        "WORKSPACE",
        "WORKSPACE.bazel",
    ]

    for f in bazel_files:
        if (project_dir / f).exists():
            files.append(f)

    # Check third_party directory
    third_party = project_dir / "third_party"
    if third_party.exists():
        for f in third_party.iterdir():
            if f.suffix in [".bzl", ".bazel"] or f.name in ["BUILD", "BUILD.bazel"]:
                files.append(f"third_party/{f.name}")

    return files


def test_repo(
    repo: RepoConfig,
    rebaze_bin: Path,
    bazel_bin: Path | None,
    work_dir: Path,
    validation_level: ValidationLevel,
    keep: bool = False,
    timeout: int = 120,
) -> TestResult:
    """Test migration for a single repository."""
    if repo.skip:
        return TestResult(
            name=repo.name,
            url=repo.url,
            success=True,
            duration_seconds=0,
            exit_code=0,
            stdout="",
            stderr="",
            skipped=True,
            skip_reason="Marked as skip in repos.toml",
            validation_level=validation_level.value,
        )

    start_time = time.monotonic()
    repo_dir = work_dir / repo.name
    validation = ValidationResult()

    # Clone
    clone_ok, clone_error = clone_repo(repo.url, repo_dir, repo.ref)
    if not clone_ok:
        return TestResult(
            name=repo.name,
            url=repo.url,
            success=False,
            duration_seconds=time.monotonic() - start_time,
            exit_code=-1,
            stdout="",
            stderr="",
            error=f"Clone failed: {clone_error}",
            validation_level=validation_level.value,
        )

    # Run migration
    dry_run = validation_level == ValidationLevel.DRY_RUN
    exit_code, stdout, stderr = run_migration(rebaze_bin, repo_dir, dry_run=dry_run, timeout=timeout)

    if exit_code != 0:
        duration = time.monotonic() - start_time
        if not keep and repo_dir.exists():
            shutil.rmtree(repo_dir, ignore_errors=True)
        return TestResult(
            name=repo.name,
            url=repo.url,
            success=False,
            duration_seconds=duration,
            exit_code=exit_code,
            stdout=stdout,
            stderr=stderr,
            error=stderr[:200] if stderr else "Migration failed",
            validation_level=validation_level.value,
        )

    # Extract/list generated files
    if dry_run:
        files_generated = extract_generated_files(stdout)
    else:
        files_generated = list_generated_files(repo_dir)

    # Validation with bazel query
    overall_success = True
    if validation_level in [ValidationLevel.QUERY, ValidationLevel.BUILD] and bazel_bin:
        query_ok, query_out, query_err, targets = run_bazel_query(bazel_bin, repo_dir)
        validation.query_success = query_ok
        validation.query_output = query_out[:1000] if query_out else ""
        validation.query_error = query_err[:500] if query_err else ""
        validation.targets_found = targets

        if not query_ok:
            overall_success = False

    # Validation with bazel build (only for header-only libraries)
    if validation_level == ValidationLevel.BUILD and bazel_bin and validation.query_success:
        if repo.header_only or "header-only" in repo.features:
            build_ok, build_out, build_err = run_bazel_build(bazel_bin, repo_dir)
            validation.build_success = build_ok
            validation.build_output = build_out[:1000] if build_out else ""
            validation.build_error = build_err[:500] if build_err else ""

            if not build_ok:
                overall_success = False
        else:
            # Skip build for non-header-only libraries (they likely have external deps)
            validation.build_success = True
            validation.build_output = "Skipped (not header-only)"

    duration = time.monotonic() - start_time

    # Cleanup if not keeping
    if not keep and repo_dir.exists():
        shutil.rmtree(repo_dir, ignore_errors=True)

    return TestResult(
        name=repo.name,
        url=repo.url,
        success=overall_success,
        duration_seconds=duration,
        exit_code=exit_code,
        stdout=stdout,
        stderr=stderr,
        files_generated=files_generated,
        validation=validation,
        validation_level=validation_level.value,
    )


def load_repos(config_path: Path) -> list[RepoConfig]:
    """Load repository configurations from TOML file."""
    content = config_path.read_text(encoding="utf-8")
    data = tomllib.loads(content)

    repos = []
    for entry in data.get("repos", []):
        features = entry.get("features", [])
        repos.append(
            RepoConfig(
                name=entry["name"],
                url=entry["url"],
                description=entry.get("description", ""),
                ref=entry.get("ref", ""),
                features=features,
                skip=entry.get("skip", False),
                header_only="header-only" in features,
            )
        )

    return repos


def print_summary(results: list[TestResult], validation_level: ValidationLevel) -> None:
    """Print a summary of test results."""
    total = len(results)
    skipped = sum(1 for r in results if r.skipped)
    tested = total - skipped
    passed = sum(1 for r in results if r.success and not r.skipped)
    failed = tested - passed

    print("\n" + "=" * 70)
    print(f"MIGRATION TEST SUMMARY (validation: {validation_level.value})")
    print("=" * 70)
    print(f"Total repos:  {total}")
    print(f"Skipped:      {skipped}")
    print(f"Tested:       {tested}")
    print(f"Passed:       {passed}")
    print(f"Failed:       {failed}")

    if validation_level != ValidationLevel.DRY_RUN:
        query_passed = sum(1 for r in results if r.validation.query_success and not r.skipped)
        print(f"\nValidation Details:")
        print(f"  Query passed:  {query_passed}/{tested}")

        if validation_level == ValidationLevel.BUILD:
            build_tested = sum(1 for r in results if not r.skipped and ("header-only" in r.name or any("header-only" in f for f in getattr(r, 'features', []))))
            build_passed = sum(1 for r in results if r.validation.build_success and not r.skipped)
            print(f"  Build passed:  {build_passed}/{tested}")

    if failed > 0:
        print("\nFailed repos:")
        for r in results:
            if not r.success and not r.skipped:
                error = r.error or r.stderr[:100]
                if r.validation.query_error:
                    error = f"Query: {r.validation.query_error[:80]}"
                elif r.validation.build_error:
                    error = f"Build: {r.validation.build_error[:80]}"
                print(f"  - {r.name}: {error}")

    print("=" * 70)


def generate_report(
    results: list[TestResult],
    output_dir: Path,
    rebaze_version: str,
    validation_level: ValidationLevel,
) -> None:
    """Generate detailed report files."""
    output_dir.mkdir(parents=True, exist_ok=True)

    # JSON report
    report_data = {
        "timestamp": datetime.now().isoformat(),
        "rebaze_version": rebaze_version,
        "validation_level": validation_level.value,
        "summary": {
            "total": len(results),
            "skipped": sum(1 for r in results if r.skipped),
            "passed": sum(1 for r in results if r.success and not r.skipped),
            "failed": sum(1 for r in results if not r.success and not r.skipped),
            "query_passed": sum(1 for r in results if r.validation.query_success and not r.skipped),
            "build_passed": sum(1 for r in results if r.validation.build_success and not r.skipped),
        },
        "results": [asdict(r) for r in results],
    }

    json_path = output_dir / "report.json"
    json_path.write_text(json.dumps(report_data, indent=2), encoding="utf-8")
    print(f"JSON report: {json_path}")

    # Markdown report
    md_lines = [
        "# Rebaze Migration Test Report",
        "",
        f"**Date:** {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}",
        f"**Rebaze Version:** {rebaze_version}",
        f"**Validation Level:** {validation_level.value}",
        "",
        "## Summary",
        "",
        "| Metric | Count |",
        "|--------|-------|",
        f"| Total | {report_data['summary']['total']} |",
        f"| Skipped | {report_data['summary']['skipped']} |",
        f"| Passed | {report_data['summary']['passed']} |",
        f"| Failed | {report_data['summary']['failed']} |",
    ]

    if validation_level != ValidationLevel.DRY_RUN:
        md_lines.extend([
            f"| Query Passed | {report_data['summary']['query_passed']} |",
            f"| Build Passed | {report_data['summary']['build_passed']} |",
        ])

    md_lines.extend([
        "",
        "## Results",
        "",
    ])

    if validation_level == ValidationLevel.DRY_RUN:
        md_lines.append("| Repo | Status | Duration | Files | Notes |")
        md_lines.append("|------|--------|----------|-------|-------|")
    else:
        md_lines.append("| Repo | Migrate | Query | Build | Duration | Targets | Notes |")
        md_lines.append("|------|---------|-------|-------|----------|---------|-------|")

    for r in results:
        if r.skipped:
            if validation_level == ValidationLevel.DRY_RUN:
                md_lines.append(f"| [{r.name}]({r.url}) | :white_circle: Skip | - | - | {r.skip_reason} |")
            else:
                md_lines.append(f"| [{r.name}]({r.url}) | :white_circle: | - | - | - | - | {r.skip_reason} |")
            continue

        migrate_status = ":white_check_mark:" if r.exit_code == 0 else ":x:"
        duration = f"{r.duration_seconds:.1f}s"

        if validation_level == ValidationLevel.DRY_RUN:
            files = str(len(r.files_generated)) if r.files_generated else "-"
            notes = (r.error or "")[:50].replace("\n", " ")
            md_lines.append(f"| [{r.name}]({r.url}) | {migrate_status} | {duration} | {files} | {notes} |")
        else:
            query_status = ":white_check_mark:" if r.validation.query_success else ":x:"
            build_status = ":white_check_mark:" if r.validation.build_success else (":yellow_circle:" if "Skipped" in r.validation.build_output else ":x:")
            targets = str(r.validation.targets_found) if r.validation.targets_found else "-"
            notes = ""
            if not r.success:
                if r.validation.query_error:
                    notes = r.validation.query_error[:40]
                elif r.validation.build_error:
                    notes = r.validation.build_error[:40]
                else:
                    notes = (r.error or r.stderr[:40]).replace("\n", " ")
            md_lines.append(f"| [{r.name}]({r.url}) | {migrate_status} | {query_status} | {build_status} | {duration} | {targets} | {notes} |")

    md_lines.extend(["", "## Failed Repos Details", ""])

    for r in results:
        if not r.success and not r.skipped:
            md_lines.extend([
                f"### {r.name}",
                "",
                f"**URL:** {r.url}",
                f"**Exit Code:** {r.exit_code}",
                "",
            ])

            if r.error:
                md_lines.extend([
                    "**Migration Error:**",
                    "```",
                    r.error[:500],
                    "```",
                    "",
                ])

            if r.validation.query_error:
                md_lines.extend([
                    "**Query Error:**",
                    "```",
                    r.validation.query_error[:500],
                    "```",
                    "",
                ])

            if r.validation.build_error and "Skipped" not in r.validation.build_output:
                md_lines.extend([
                    "**Build Error:**",
                    "```",
                    r.validation.build_error[:500],
                    "```",
                    "",
                ])

    md_path = output_dir / "report.md"
    md_path.write_text("\n".join(md_lines), encoding="utf-8")
    print(f"Markdown report: {md_path}")


def get_rebaze_version(rebaze_bin: Path) -> str:
    """Get rebaze version string."""
    try:
        result = subprocess.run(
            [str(rebaze_bin), "--version"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return result.stdout.strip() or "unknown"
    except Exception:
        return "unknown"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Test rebaze migration against well-known CMake projects"
    )
    parser.add_argument(
        "--repos",
        type=Path,
        default=Path(__file__).parent / "repos.toml",
        help="Path to repos.toml configuration",
    )
    parser.add_argument(
        "--filter",
        type=str,
        default="",
        help="Comma-separated list of repo names to test (default: all)",
    )
    parser.add_argument(
        "--parallel",
        "-j",
        type=int,
        default=1,
        help="Number of parallel tests (default: 1)",
    )
    parser.add_argument(
        "--keep",
        action="store_true",
        help="Keep cloned repositories after testing",
    )
    parser.add_argument(
        "--work-dir",
        type=Path,
        default=None,
        help="Working directory for clones (default: temp dir)",
    )
    parser.add_argument(
        "--output",
        "-o",
        type=Path,
        default=Path(__file__).parent / "reports",
        help="Output directory for reports",
    )
    parser.add_argument(
        "--rebaze",
        type=Path,
        default=None,
        help="Path to rebaze binary (default: auto-detect)",
    )
    parser.add_argument(
        "--bazel",
        type=Path,
        default=None,
        help="Path to bazel/bazelisk binary (default: auto-detect)",
    )
    parser.add_argument(
        "--include-skipped",
        action="store_true",
        help="Include repos marked as skip=true",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=120,
        help="Timeout per migration in seconds (default: 120)",
    )
    parser.add_argument(
        "--validate",
        action="store_true",
        help="Generate files and validate with bazel query //...",
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="Also attempt bazel build //... (implies --validate, only for header-only libs)",
    )

    args = parser.parse_args()

    # Determine validation level
    if args.build:
        validation_level = ValidationLevel.BUILD
    elif args.validate:
        validation_level = ValidationLevel.QUERY
    else:
        validation_level = ValidationLevel.DRY_RUN

    # Find rebaze binary
    rebaze_bin = args.rebaze or find_rebaze_binary()
    if not rebaze_bin:
        print("Error: Could not find rebaze binary.", file=sys.stderr)
        print("Build with: cargo build --release", file=sys.stderr)
        return 1

    rebaze_bin = Path(rebaze_bin).resolve()
    if not rebaze_bin.exists():
        print(f"Error: Rebaze binary not found: {rebaze_bin}", file=sys.stderr)
        return 1

    print(f"Using rebaze: {rebaze_bin}")
    rebaze_version = get_rebaze_version(rebaze_bin)
    print(f"Version: {rebaze_version}")

    # Find bazel binary (required for validation)
    bazel_bin = None
    if validation_level != ValidationLevel.DRY_RUN:
        bazel_bin = args.bazel or find_bazel_binary()
        if not bazel_bin:
            print("Error: Could not find bazel/bazelisk binary.", file=sys.stderr)
            print("Install bazelisk: brew install bazelisk", file=sys.stderr)
            return 1
        bazel_bin = Path(bazel_bin).resolve()
        print(f"Using bazel: {bazel_bin}")

    # Load repos
    if not args.repos.exists():
        print(f"Error: Repos config not found: {args.repos}", file=sys.stderr)
        return 1

    repos = load_repos(args.repos)
    print(f"Loaded {len(repos)} repos from {args.repos}")

    # Filter repos
    if args.filter:
        filter_names = set(name.strip() for name in args.filter.split(","))
        repos = [r for r in repos if r.name in filter_names]
        print(f"Filtered to {len(repos)} repos: {', '.join(r.name for r in repos)}")

    if not repos:
        print("No repos to test.", file=sys.stderr)
        return 1

    # Setup working directory
    if args.work_dir:
        work_dir = args.work_dir.resolve()
        work_dir.mkdir(parents=True, exist_ok=True)
        cleanup_work_dir = False
    else:
        work_dir = Path(tempfile.mkdtemp(prefix="rebaze-test-"))
        cleanup_work_dir = not args.keep

    print(f"Working directory: {work_dir}")
    print(f"Validation level: {validation_level.value}")
    print(f"Testing {len(repos)} repos (parallel={args.parallel})...")
    print()

    # Run tests
    results: list[TestResult] = []

    if args.parallel > 1:
        with ThreadPoolExecutor(max_workers=args.parallel) as executor:
            futures = {
                executor.submit(
                    test_repo, repo, rebaze_bin, bazel_bin, work_dir,
                    validation_level, args.keep, args.timeout
                ): repo
                for repo in repos
            }
            for future in as_completed(futures):
                repo = futures[future]
                try:
                    result = future.result()
                except Exception as e:
                    result = TestResult(
                        name=repo.name,
                        url=repo.url,
                        success=False,
                        duration_seconds=0,
                        exit_code=-1,
                        stdout="",
                        stderr="",
                        error=str(e),
                        validation_level=validation_level.value,
                    )
                results.append(result)

                if result.skipped:
                    status = "SKIP"
                elif result.success:
                    if validation_level != ValidationLevel.DRY_RUN:
                        status = f"PASS (targets: {result.validation.targets_found})"
                    else:
                        status = "PASS"
                else:
                    status = "FAIL"
                print(f"  [{status}] {result.name} ({result.duration_seconds:.1f}s)")
    else:
        for repo in repos:
            result = test_repo(
                repo, rebaze_bin, bazel_bin, work_dir,
                validation_level, args.keep, args.timeout
            )
            results.append(result)

            if result.skipped:
                status = "SKIP"
            elif result.success:
                if validation_level != ValidationLevel.DRY_RUN:
                    status = f"PASS (targets: {result.validation.targets_found})"
                else:
                    status = "PASS"
            else:
                status = "FAIL"
            print(f"  [{status}] {result.name} ({result.duration_seconds:.1f}s)")

    # Sort results by name for consistent output
    results.sort(key=lambda r: r.name)

    # Print summary
    print_summary(results, validation_level)

    # Generate reports
    generate_report(results, args.output, rebaze_version, validation_level)

    # Cleanup
    if cleanup_work_dir and work_dir.exists():
        shutil.rmtree(work_dir, ignore_errors=True)

    # Return non-zero if any tests failed
    failed = sum(1 for r in results if not r.success and not r.skipped)
    return 1 if failed > 0 else 0


if __name__ == "__main__":
    raise SystemExit(main())
