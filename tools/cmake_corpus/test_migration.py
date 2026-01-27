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
3. Runs `rebaze migrate --dry-run` on each
4. Collects results and generates a report

Usage:
    ./test_migration.py                    # Test all repos
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


@dataclass
class RepoConfig:
    """Configuration for a repository to test."""

    name: str
    url: str
    description: str = ""
    ref: str = ""
    features: list[str] = field(default_factory=list)
    skip: bool = False


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
    timeout: int = 120,
) -> tuple[int, str, str]:
    """Run rebaze migrate --dry-run on a project."""
    cmd = [str(rebaze_bin), "migrate", "--dry-run", "--unsafe-mode", str(project_dir)]

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


def test_repo(
    repo: RepoConfig,
    rebaze_bin: Path,
    work_dir: Path,
    keep: bool = False,
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
        )

    start_time = time.monotonic()
    repo_dir = work_dir / repo.name

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
        )

    # Run migration
    exit_code, stdout, stderr = run_migration(rebaze_bin, repo_dir)
    duration = time.monotonic() - start_time

    # Extract generated files
    files_generated = extract_generated_files(stdout) if exit_code == 0 else []

    # Cleanup if not keeping
    if not keep and repo_dir.exists():
        shutil.rmtree(repo_dir, ignore_errors=True)

    return TestResult(
        name=repo.name,
        url=repo.url,
        success=exit_code == 0,
        duration_seconds=duration,
        exit_code=exit_code,
        stdout=stdout,
        stderr=stderr,
        files_generated=files_generated,
    )


def load_repos(config_path: Path) -> list[RepoConfig]:
    """Load repository configurations from TOML file."""
    content = config_path.read_text(encoding="utf-8")
    data = tomllib.loads(content)

    repos = []
    for entry in data.get("repos", []):
        repos.append(
            RepoConfig(
                name=entry["name"],
                url=entry["url"],
                description=entry.get("description", ""),
                ref=entry.get("ref", ""),
                features=entry.get("features", []),
                skip=entry.get("skip", False),
            )
        )

    return repos


def print_summary(results: list[TestResult]) -> None:
    """Print a summary of test results."""
    total = len(results)
    skipped = sum(1 for r in results if r.skipped)
    tested = total - skipped
    passed = sum(1 for r in results if r.success and not r.skipped)
    failed = tested - passed

    print("\n" + "=" * 60)
    print("MIGRATION TEST SUMMARY")
    print("=" * 60)
    print(f"Total repos:  {total}")
    print(f"Skipped:      {skipped}")
    print(f"Tested:       {tested}")
    print(f"Passed:       {passed}")
    print(f"Failed:       {failed}")

    if failed > 0:
        print("\nFailed repos:")
        for r in results:
            if not r.success and not r.skipped:
                print(f"  - {r.name}: {r.error or r.stderr[:100]}")

    print("=" * 60)


def generate_report(
    results: list[TestResult],
    output_dir: Path,
    rebaze_version: str,
) -> None:
    """Generate detailed report files."""
    output_dir.mkdir(parents=True, exist_ok=True)

    # JSON report
    report_data = {
        "timestamp": datetime.now().isoformat(),
        "rebaze_version": rebaze_version,
        "summary": {
            "total": len(results),
            "skipped": sum(1 for r in results if r.skipped),
            "passed": sum(1 for r in results if r.success and not r.skipped),
            "failed": sum(1 for r in results if not r.success and not r.skipped),
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
        "",
        "## Summary",
        "",
        f"| Metric | Count |",
        f"|--------|-------|",
        f"| Total | {report_data['summary']['total']} |",
        f"| Skipped | {report_data['summary']['skipped']} |",
        f"| Passed | {report_data['summary']['passed']} |",
        f"| Failed | {report_data['summary']['failed']} |",
        "",
        "## Results",
        "",
        "| Repo | Status | Duration | Files | Notes |",
        "|------|--------|----------|-------|-------|",
    ]

    for r in results:
        if r.skipped:
            status = ":white_circle: Skip"
            notes = r.skip_reason
        elif r.success:
            status = ":white_check_mark: Pass"
            notes = ""
        else:
            status = ":x: Fail"
            notes = (r.error or r.stderr[:50]).replace("\n", " ")

        duration = f"{r.duration_seconds:.1f}s" if not r.skipped else "-"
        files = str(len(r.files_generated)) if r.files_generated else "-"

        md_lines.append(f"| [{r.name}]({r.url}) | {status} | {duration} | {files} | {notes} |")

    md_lines.extend(["", "## Failed Repos Details", ""])

    for r in results:
        if not r.success and not r.skipped:
            md_lines.extend(
                [
                    f"### {r.name}",
                    "",
                    f"**URL:** {r.url}",
                    f"**Exit Code:** {r.exit_code}",
                    "",
                    "**Error:**",
                    "```",
                    r.error or r.stderr[:500],
                    "```",
                    "",
                ]
            )

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

    args = parser.parse_args()

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

    if not args.include_skipped:
        # Don't remove skipped repos, just let them be marked as skipped in results
        pass

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
    print(f"Testing {len(repos)} repos (parallel={args.parallel})...")
    print()

    # Run tests
    results: list[TestResult] = []

    if args.parallel > 1:
        with ThreadPoolExecutor(max_workers=args.parallel) as executor:
            futures = {
                executor.submit(test_repo, repo, rebaze_bin, work_dir, args.keep): repo
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
                    )
                results.append(result)
                status = (
                    "SKIP"
                    if result.skipped
                    else ("PASS" if result.success else "FAIL")
                )
                print(f"  [{status}] {result.name} ({result.duration_seconds:.1f}s)")
    else:
        for repo in repos:
            result = test_repo(repo, rebaze_bin, work_dir, args.keep)
            results.append(result)
            status = (
                "SKIP" if result.skipped else ("PASS" if result.success else "FAIL")
            )
            print(f"  [{status}] {result.name} ({result.duration_seconds:.1f}s)")

    # Sort results by name for consistent output
    results.sort(key=lambda r: r.name)

    # Print summary
    print_summary(results)

    # Generate reports
    generate_report(results, args.output, rebaze_version)

    # Cleanup
    if cleanup_work_dir and work_dir.exists():
        shutil.rmtree(work_dir, ignore_errors=True)

    # Return non-zero if any tests failed
    failed = sum(1 for r in results if not r.success and not r.skipped)
    return 1 if failed > 0 else 0


if __name__ == "__main__":
    raise SystemExit(main())
