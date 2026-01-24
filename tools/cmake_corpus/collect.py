#!/usr/bin/env -S uv run --script
#
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Collect a corpus of CMake files from GitHub using gh search, then sparse-checkout
those files for parser testing.
"""

from __future__ import annotations

import argparse
import calendar
import json
import os
import subprocess
import sys
import time
import tomllib
import tempfile
from collections import defaultdict
from dataclasses import dataclass
from datetime import date, timedelta
from pathlib import Path
from typing import Iterable, List


@dataclass(frozen=True)
class SearchItem:
    repo: str
    path: str
    html_url: str


RATE_LIMIT_WAIT = False
RATE_LIMIT_SLEEP = 0.0

DEFAULT_DENY_LICENSES = {
    "AGPL-3.0",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
}

DEFAULT_CONFIG = {
    "count": 100,
    "start_year": 2010,
    "max_per_repo": 3,
    "per_page": 100,
    "query_extra": "",
    "no_fetch": False,
    "wait": False,
    "sleep": 0.0,
    "allow_license": [],
    "deny_license": sorted(DEFAULT_DENY_LICENSES),
    "skip_unknown_license": False,
}


def run(cmd: List[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, check=True, text=True, stdout=subprocess.PIPE)


def find_repo_root(start: Path) -> Path | None:
    markers = [".git", "AGENTS.md", "Cargo.toml", "MODULE.bazel"]
    for candidate in [start, *start.parents]:
        if any((candidate / marker).exists() for marker in markers):
            return candidate
    return None


def default_corpus_dir() -> Path:
    env_override = os.getenv("REBAZE_CMAKE_CORPUS_DIR")
    if env_override:
        return Path(env_override)
    try:
        cwd = Path.cwd()
    except FileNotFoundError:
        cwd = None
    repo_root = find_repo_root(cwd) if cwd else None
    if repo_root:
        return repo_root / ".rebaze" / "cmake-corpus"
    xdg_cache = os.getenv("XDG_CACHE_HOME")
    if xdg_cache:
        return Path(xdg_cache) / "rebaze" / "cmake-corpus"
    return Path(tempfile.gettempdir()) / "rebaze-cmake-corpus"


def gh_search(query: str, page: int, per_page: int) -> dict:
    ensure_search_budget(RATE_LIMIT_WAIT, RATE_LIMIT_SLEEP)
    result = run(
        [
            "gh",
            "api",
            "-X",
            "GET",
            "/search/code",
            "-f",
            f"q={query}",
            "-f",
            f"per_page={per_page}",
            "-f",
            f"page={page}",
        ]
    )
    return json.loads(result.stdout)


def gh_repo_license(repo: str) -> str:
    result = run(["gh", "api", f"/repos/{repo}"])
    data = json.loads(result.stdout)
    license_info = data.get("license")
    if not license_info:
        return "UNKNOWN"
    spdx = license_info.get("spdx_id")
    return spdx or "UNKNOWN"


def gh_rate_limit() -> dict:
    result = run(["gh", "api", "/rate_limit"])
    return json.loads(result.stdout)


def ensure_search_budget(wait: bool, sleep_seconds: float) -> None:
    data = gh_rate_limit()
    search = data.get("resources", {}).get("search", {})
    remaining = int(search.get("remaining", 0))
    reset = int(search.get("reset", 0))
    if remaining > 0:
        if sleep_seconds > 0:
            time.sleep(sleep_seconds)
        return
    if not wait:
        raise RuntimeError("GitHub search rate limit exceeded; rerun with --wait")
    now = int(time.time())
    delay = max(0, reset - now + 5)
    if delay > 0:
        print(f"Rate limit exceeded. Sleeping {delay}s...", file=sys.stderr)
        time.sleep(delay)


def total_count_for(query: str) -> int:
    data = gh_search(query, page=1, per_page=1)
    return int(data.get("total_count", 0))


def iter_ranges(start_year: int) -> Iterable[tuple[date, date]]:
    today = date.today()
    for year in range(today.year, start_year - 1, -1):
        start = date(year, 1, 1)
        end = date(year, 12, 31)
        yield start, end


def split_range(start: date, end: date) -> Iterable[tuple[date, date]]:
    if start == end:
        yield start, end
        return
    mid = start + (end - start) // 2
    yield start, mid
    yield mid + timedelta(days=1), end


def iter_months(start: date, end: date) -> Iterable[tuple[date, date]]:
    cursor = date(start.year, start.month, 1)
    while cursor <= end:
        last_day = calendar.monthrange(cursor.year, cursor.month)[1]
        month_end = date(cursor.year, cursor.month, last_day)
        yield cursor, month_end
        if cursor.month == 12:
            cursor = date(cursor.year + 1, 1, 1)
        else:
            cursor = date(cursor.year, cursor.month + 1, 1)


def query_for_range(start: date, end: date, extra: str) -> str:
    date_range = f"created:{start.isoformat()}..{end.isoformat()}"
    parts = ["filename:CMakeLists.txt", date_range]
    if extra:
        parts.append(extra)
    return " ".join(parts)


def fetch_items_for_range(
    start: date,
    end: date,
    extra: str,
    license_cache: dict[str, str],
    allow_licenses: set[str],
    deny_licenses: set[str],
    skip_unknown_license: bool,
    skipped_by_license: dict[str, int],
    selected: List[SearchItem],
    seen: set[tuple[str, str]],
    per_repo: dict[str, int],
    max_per_repo: int,
    target_count: int,
    per_page: int,
) -> None:
    query = query_for_range(start, end, extra)
    total = total_count_for(query)
    if total == 0:
        return
    if total > 1000 and (end - start).days > 0:
        for sub_start, sub_end in split_range(start, end):
            fetch_items_for_range(
                sub_start,
                sub_end,
                extra,
                license_cache,
                allow_licenses,
                deny_licenses,
                skip_unknown_license,
                skipped_by_license,
                selected,
                seen,
                per_repo,
                max_per_repo,
                target_count,
                per_page,
            )
            if len(selected) >= target_count:
                return
        return

    page = 1
    while True:
        data = gh_search(query, page=page, per_page=per_page)
        items = data.get("items", [])
        if not items:
            return
        for item in items:
            repo = item["repository"]["full_name"]
            path = item["path"]
            key = (repo, path)
            if key in seen:
                continue
            if per_repo[repo] >= max_per_repo:
                continue
            if repo not in license_cache:
                license_cache[repo] = gh_repo_license(repo)
            license_id = license_cache[repo]
            if skip_unknown_license and license_id == "UNKNOWN":
                skipped_by_license["UNKNOWN"] += 1
                continue
            if allow_licenses and license_id not in allow_licenses:
                skipped_by_license[license_id] += 1
                continue
            if deny_licenses and license_id in deny_licenses:
                skipped_by_license[license_id] += 1
                continue
            selected.append(
                SearchItem(repo=repo, path=path, html_url=item.get("html_url", ""))
            )
            seen.add(key)
            per_repo[repo] += 1
            if len(selected) >= target_count:
                return
        if len(items) < per_page:
            return
        page += 1


def ensure_sparse_checkout(repo_dir: Path, repo: str, paths: List[str]) -> None:
    if not repo_dir.exists():
        run(
            [
                "git",
                "clone",
                "--depth=1",
                "--filter=blob:none",
                "--sparse",
                f"https://github.com/{repo}.git",
                str(repo_dir),
            ]
        )
    run(["git", "-C", str(repo_dir), "sparse-checkout", "init", "--no-cone"])
    run(["git", "-C", str(repo_dir), "sparse-checkout", "set", "--no-cone", *paths])


def load_config(path: Path | None) -> dict:
    if not path or not path.exists():
        return {}
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise RuntimeError(f"Invalid TOML in {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise RuntimeError(f"Invalid config in {path}: expected a TOML table")
    return data


def ensure_list(value: object) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        return [str(item) for item in value]
    if isinstance(value, str):
        return [value]
    raise RuntimeError("Expected a list or string for license configuration.")


def normalize_optional_path(value: object) -> Path | None:
    if value is None:
        return None
    if isinstance(value, Path):
        return value
    if isinstance(value, str):
        stripped = value.strip()
        if not stripped:
            return None
        expanded = os.path.expandvars(stripped)
        return Path(expanded).expanduser()
    raise RuntimeError("Expected a string path for configuration.")


def main() -> int:
    pre_parser = argparse.ArgumentParser(add_help=False)
    pre_parser.add_argument("--config", type=Path)
    pre_args, _ = pre_parser.parse_known_args()
    config_path = pre_args.config
    if config_path is not None and not config_path.exists():
        print(f"Config file not found: {config_path}", file=sys.stderr)
        return 2
    default_config_path = Path("tools/cmake_corpus/config.toml")
    if config_path is None and default_config_path.exists():
        config_path = default_config_path
    try:
        config_data = load_config(config_path)
    except RuntimeError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    def cfg(key: str, default: object) -> object:
        return config_data.get(key, default)

    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, default=config_path)
    parser.add_argument(
        "--corpus-dir",
        dest="corpus_dir",
        type=Path,
        default=None,
        help="Base directory for outputs (manifest, files, cache, and repos).",
    )
    parser.add_argument(
        "--artifacts-dir",
        dest="corpus_dir",
        type=Path,
        default=None,
        help=argparse.SUPPRESS,
    )
    parser.add_argument("--count", type=int, default=cfg("count", DEFAULT_CONFIG["count"]))
    parser.add_argument(
        "--start-year", type=int, default=cfg("start_year", DEFAULT_CONFIG["start_year"])
    )
    parser.add_argument(
        "--workdir",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Output file list (one path per line).",
    )
    parser.add_argument(
        "--max-per-repo",
        type=int,
        default=cfg("max_per_repo", DEFAULT_CONFIG["max_per_repo"]),
    )
    parser.add_argument(
        "--per-page", type=int, default=cfg("per_page", DEFAULT_CONFIG["per_page"])
    )
    parser.add_argument(
        "--query-extra", default=cfg("query_extra", DEFAULT_CONFIG["query_extra"])
    )
    parser.add_argument("--no-fetch", dest="no_fetch", action="store_true", default=None)
    parser.add_argument("--fetch", dest="no_fetch", action="store_false")
    parser.add_argument("--wait", dest="wait", action="store_true", default=None)
    parser.add_argument("--no-wait", dest="wait", action="store_false")
    parser.add_argument("--sleep", type=float, default=cfg("sleep", DEFAULT_CONFIG["sleep"]))
    parser.add_argument(
        "--allow-license",
        action="append",
        default=None,
        help="Only include repositories with these SPDX IDs (repeatable).",
    )
    parser.add_argument(
        "--deny-license",
        action="append",
        default=None,
        help="Exclude repositories with these SPDX IDs (repeatable).",
    )
    parser.add_argument(
        "--skip-unknown-license",
        dest="skip_unknown_license",
        action="store_true",
        default=None,
        help="Skip repositories with unknown license metadata.",
    )
    parser.add_argument(
        "--allow-unknown-license",
        dest="skip_unknown_license",
        action="store_false",
    )
    parser.add_argument(
        "--license-cache",
        type=Path,
        default=None,
    )
    args = parser.parse_args()

    corpus_dir_cli = args.corpus_dir
    corpus_dir_cfg = normalize_optional_path(
        cfg("corpus_dir", cfg("artifacts_dir", None))
    )
    corpus_dir = (
        normalize_optional_path(corpus_dir_cli)
        or corpus_dir_cfg
        or default_corpus_dir()
    )

    if args.workdir is not None:
        workdir = normalize_optional_path(args.workdir)
    elif corpus_dir_cli is not None:
        workdir = corpus_dir
    else:
        workdir = normalize_optional_path(cfg("workdir", None)) or corpus_dir
    if workdir is None:
        workdir = default_corpus_dir()

    if args.out is not None:
        out_path = normalize_optional_path(args.out)
    elif corpus_dir_cli is not None:
        out_path = None
    else:
        out_path = normalize_optional_path(cfg("out", None))
    if out_path is None:
        out_path = workdir / "files.txt"

    if args.license_cache is not None:
        license_cache_path = normalize_optional_path(args.license_cache)
    elif corpus_dir_cli is not None:
        license_cache_path = None
    else:
        license_cache_path = normalize_optional_path(cfg("license_cache", None))
    if license_cache_path is None:
        license_cache_path = workdir / "license_cache.json"

    if args.no_fetch is None:
        args.no_fetch = bool(cfg("no_fetch", DEFAULT_CONFIG["no_fetch"]))
    if args.wait is None:
        args.wait = bool(cfg("wait", DEFAULT_CONFIG["wait"]))
    if args.skip_unknown_license is None:
        args.skip_unknown_license = bool(
            cfg("skip_unknown_license", DEFAULT_CONFIG["skip_unknown_license"])
        )
    if args.allow_license is None:
        args.allow_license = ensure_list(cfg("allow_license", []))
    if args.deny_license is None:
        args.deny_license = ensure_list(cfg("deny_license", DEFAULT_CONFIG["deny_license"]))

    global RATE_LIMIT_WAIT
    global RATE_LIMIT_SLEEP
    RATE_LIMIT_WAIT = args.wait
    RATE_LIMIT_SLEEP = args.sleep

    if args.count <= 0:
        print("count must be > 0", file=sys.stderr)
        return 2

    repos_dir = workdir / "repos"
    workdir.mkdir(parents=True, exist_ok=True)
    repos_dir.mkdir(parents=True, exist_ok=True)

    allow_licenses = set(args.allow_license)
    deny_licenses = set(args.deny_license)
    skipped_by_license: dict[str, int] = defaultdict(int)

    license_cache: dict[str, str] = {}
    if license_cache_path.exists():
        try:
            license_cache = json.loads(
                license_cache_path.read_text(encoding="utf-8")
            )
        except json.JSONDecodeError:
            license_cache = {}

    selected: List[SearchItem] = []
    seen: set[tuple[str, str]] = set()
    per_repo: dict[str, int] = defaultdict(int)

    for start, end in iter_ranges(args.start_year):
        fetch_items_for_range(
            start,
            end,
            args.query_extra,
            license_cache,
            allow_licenses,
            deny_licenses,
            args.skip_unknown_license,
            skipped_by_license,
            selected,
            seen,
            per_repo,
            args.max_per_repo,
            args.count,
            args.per_page,
        )
        if len(selected) >= args.count:
            break

    if not selected:
        print("No files found. Try adjusting --start-year or --query-extra.", file=sys.stderr)
        return 1

    license_cache_path.parent.mkdir(parents=True, exist_ok=True)
    license_cache_path.write_text(
        json.dumps(license_cache, indent=2), encoding="utf-8"
    )

    manifest_path = workdir / "manifest.jsonl"
    with manifest_path.open("w", encoding="utf-8") as manifest:
        for item in selected:
            manifest.write(json.dumps(item.__dict__) + "\n")

    if args.no_fetch:
        print(f"Wrote manifest: {manifest_path}")
        return 0

    grouped: dict[str, List[str]] = defaultdict(list)
    for item in selected:
        grouped[item.repo].append(item.path)

    file_list: List[str] = []
    for repo, paths in grouped.items():
        repo_dir = repos_dir / repo.replace("/", "_")
        ensure_sparse_checkout(repo_dir, repo, paths)
        for path in paths:
            file_path = repo_dir / path
            if file_path.exists():
                file_list.append(str(file_path))

    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w", encoding="utf-8") as out:
        for path in file_list:
            out.write(f"{path}\n")

    print(f"Wrote manifest: {manifest_path}")
    print(f"Wrote file list: {out_path}")
    if skipped_by_license:
        print("Skipped by license:")
        for lic, count in sorted(skipped_by_license.items()):
            print(f"  {lic}: {count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
