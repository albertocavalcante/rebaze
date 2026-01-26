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
import json
import os
import shutil
import subprocess
import sys
import time
import tomllib
import tempfile
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import List


@dataclass(frozen=True)
class SearchItem:
    repo: str
    path: str
    html_url: str


RATE_LIMIT_WAIT = False
RATE_LIMIT_SLEEP = 0.0
RATE_LIMIT_MIN_INTERVAL = 10.0
RATE_LIMIT_LAST_CHECK = 0.0
RATE_LIMIT_LAST_DATA: dict | None = None

SEARCH_CACHE_ENABLED = True
SEARCH_CACHE_TTL_SECONDS = 0
SEARCH_CACHE_PATH: Path | None = None
SEARCH_CACHE: dict[str, dict] = {}
SEARCH_CACHE_HITS = 0
SEARCH_CACHE_MISSES = 0

DEFAULT_DENY_LICENSES = {
    "AGPL-3.0",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
}

DEFAULT_CONFIG = {
    "count": 100,
    "max_per_repo": 3,
    "per_page": 100,
    "query_extra": "",
    "no_fetch": False,
    "wait": False,
    "sleep": 0.0,
    "allow_license": [],
    "deny_license": sorted(DEFAULT_DENY_LICENSES),
    "skip_unknown_license": False,
    "exact_basename": True,
    "filter_invalid": True,
    "search_backend": "github",
    "search_limit": 0,
    "cache_enabled": True,
    "cache_ttl_seconds": 86_400,
}


def run(cmd: List[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


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


def cache_get(key: str) -> dict | None:
    global SEARCH_CACHE_HITS
    global SEARCH_CACHE_MISSES
    if not SEARCH_CACHE_ENABLED:
        return None
    entry = SEARCH_CACHE.get(key)
    if not entry:
        SEARCH_CACHE_MISSES += 1
        return None
    ts = entry.get("ts")
    data = entry.get("data")
    if not isinstance(ts, (int, float)) or data is None:
        SEARCH_CACHE_MISSES += 1
        return None
    if SEARCH_CACHE_TTL_SECONDS > 0:
        if time.time() - float(ts) > SEARCH_CACHE_TTL_SECONDS:
            SEARCH_CACHE_MISSES += 1
            return None
    SEARCH_CACHE_HITS += 1
    return data


def cache_set(key: str, data: dict) -> None:
    if not SEARCH_CACHE_ENABLED:
        return
    SEARCH_CACHE[key] = {"ts": int(time.time()), "data": data}


def gh_search(query: str, page: int, per_page: int) -> dict:
    cache_key = f"{query}|page={page}|per_page={per_page}"
    cached = cache_get(cache_key)
    if cached is not None:
        return cached
    while True:
        ensure_search_budget(RATE_LIMIT_WAIT, RATE_LIMIT_SLEEP)
        try:
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
            data = json.loads(result.stdout)
            cache_set(cache_key, data)
            return data
        except subprocess.CalledProcessError as exc:
            if RATE_LIMIT_WAIT and is_rate_limit_error(exc):
                ensure_search_budget(True, RATE_LIMIT_SLEEP)
                continue
            raise


def gh_repo_license(repo: str) -> str:
    while True:
        try:
            result = run(["gh", "api", f"/repos/{repo}"])
        except subprocess.CalledProcessError as exc:
            if RATE_LIMIT_WAIT and is_rate_limit_error(exc):
                ensure_search_budget(True, RATE_LIMIT_SLEEP)
                continue
            raise
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
    global RATE_LIMIT_LAST_CHECK
    global RATE_LIMIT_LAST_DATA

    now = time.time()
    if RATE_LIMIT_LAST_DATA and now - RATE_LIMIT_LAST_CHECK < RATE_LIMIT_MIN_INTERVAL:
        data = RATE_LIMIT_LAST_DATA
    else:
        data = gh_rate_limit()
        RATE_LIMIT_LAST_DATA = data
        RATE_LIMIT_LAST_CHECK = now
    resources = data.get("resources", {})
    search = resources.get("code_search") or resources.get("search", {})
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


def is_rate_limit_error(exc: subprocess.CalledProcessError) -> bool:
    stderr = (exc.stderr or "").lower()
    stdout = (exc.stdout or "").lower()
    return "rate limit" in stderr or "rate limit" in stdout


def total_count_for(query: str) -> int:
    data = gh_search(query, page=1, per_page=1)
    return int(data.get("total_count", 0))

def query_for_size_range(min_size: int, max_size: int, extra: str) -> str:
    size_range = f"size:{min_size}..{max_size}"
    parts = ["filename:CMakeLists.txt", size_range]
    if extra:
        parts.append(extra)
    return " ".join(parts)


def query_for_min_size(min_size: int, extra: str) -> str:
    parts = ["filename:CMakeLists.txt", f"size:>{min_size}"]
    if extra:
        parts.append(extra)
    return " ".join(parts)


def fetch_items_for_size_range(
    min_size: int,
    max_size: int,
    extra: str,
    license_cache: dict[str, str],
    allow_licenses: set[str],
    deny_licenses: set[str],
    skip_unknown_license: bool,
    skipped_by_license: dict[str, int],
    exact_basename: bool,
    skipped_by_name: dict[str, int],
    skip_repos: set[str],
    skip_paths: set[str],
    skipped_by_skiplist: dict[str, int],
    selected: List[SearchItem],
    seen: set[tuple[str, str]],
    per_repo: dict[str, int],
    max_per_repo: int,
    target_count: int,
    per_page: int,
) -> None:
    if min_size > max_size:
        return
    query = query_for_size_range(min_size, max_size, extra)
    total = total_count_for(query)
    if total == 0:
        return
    if total > 1000 and min_size < max_size:
        mid = min_size + (max_size - min_size) // 2
        fetch_items_for_size_range(
            min_size,
            mid,
            extra,
            license_cache,
            allow_licenses,
            deny_licenses,
            skip_unknown_license,
            skipped_by_license,
            exact_basename,
            skipped_by_name,
            skip_repos,
            skip_paths,
            skipped_by_skiplist,
            selected,
            seen,
            per_repo,
            max_per_repo,
            target_count,
            per_page,
        )
        if len(selected) >= target_count:
            return
        fetch_items_for_size_range(
            mid + 1,
            max_size,
            extra,
            license_cache,
            allow_licenses,
            deny_licenses,
            skip_unknown_license,
            skipped_by_license,
            exact_basename,
            skipped_by_name,
            skip_repos,
            skip_paths,
            skipped_by_skiplist,
            selected,
            seen,
            per_repo,
            max_per_repo,
            target_count,
            per_page,
        )
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
            if exact_basename and not is_cmakelists_path(path):
                skipped_by_name["basename_mismatch"] += 1
                continue
            if repo in skip_repos:
                skipped_by_skiplist["repo"] += 1
                continue
            repo_path = f"{repo}/{path}"
            if repo_path in skip_paths:
                skipped_by_skiplist["path"] += 1
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


def read_text_lossy(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return path.read_text(encoding="utf-8", errors="replace")


def ensure_list(value: object) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        return [str(item) for item in value]
    if isinstance(value, str):
        return [value]
    raise RuntimeError("Expected a list or string for license configuration.")


def normalize_optional_path(value: object, base: Path | None = None) -> Path | None:
    if value is None:
        return None
    if isinstance(value, Path):
        path = value
        if base and not path.is_absolute():
            return base / path
        return path
    if isinstance(value, str):
        stripped = value.strip()
        if not stripped:
            return None
        expanded = os.path.expandvars(stripped)
        path = Path(expanded).expanduser()
        if base and not path.is_absolute():
            return base / path
        return path
    raise RuntimeError("Expected a string path for configuration.")


def normalize_optional_int(value: object) -> int | None:
    if value is None:
        return None
    if isinstance(value, bool):
        raise RuntimeError("Expected an integer for size configuration.")
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        stripped = value.strip()
        if not stripped:
            return None
        try:
            return int(stripped)
        except ValueError as exc:
            raise RuntimeError(
                f"Expected an integer for size configuration, got '{value}'."
            ) from exc
    raise RuntimeError("Expected an integer for size configuration.")


def resolve_size_max(min_size: int, target_count: int, extra: str) -> int:
    candidate = max(min_size + 1, 1024)
    while True:
        total = total_count_for(query_for_size_range(min_size, candidate, extra))
        if total >= target_count:
            return candidate
        overflow = total_count_for(query_for_min_size(candidate, extra))
        if overflow == 0:
            return candidate
        candidate *= 2


def is_cmakelists_path(path: str) -> bool:
    name = Path(path).name
    return name.casefold() == "cmakelists.txt"


def sourcegraph_query(extra: str) -> str:
    parts = ["file:CMakeLists.txt", "patternType:literal"]
    if extra:
        parts.append(extra)
    return " ".join(parts)


def normalize_sourcegraph_repo(repo: str | None) -> str | None:
    if not repo:
        return None
    normalized = repo.strip().split("@", 1)[0]
    if normalized.startswith("github.com/"):
        return normalized[len("github.com/") :]
    first = normalized.split("/", 1)[0]
    if "." not in first and "/" in normalized:
        return normalized
    return None


def sourcegraph_search(query: str, limit: int) -> tuple[list[SearchItem], dict[str, int]]:
    skipped: dict[str, int] = defaultdict(int)
    cmd = ["src", "search", "-json", "-stream"]
    if limit > 0:
        cmd.extend(["-display", str(limit)])
    cmd.extend(["--", query])
    try:
        result = run(cmd)
    except subprocess.CalledProcessError as exc:
        stderr = (exc.stderr or "").strip()
        raise RuntimeError(f"Sourcegraph search failed: {stderr}") from exc
    items: list[SearchItem] = []
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            skipped["invalid_json"] += 1
            continue
        if not isinstance(event, dict):
            continue
        if event.get("type") == "match":
            data = event.get("data", {})
        else:
            data = event
        if not isinstance(data, dict):
            continue
        path = data.get("path")
        repo = data.get("repository")
        if isinstance(repo, dict):
            repo = repo.get("name") or repo.get("repo") or repo.get("repository")
        if not path or not isinstance(path, str):
            skipped["missing_path"] += 1
            continue
        repo_norm = normalize_sourcegraph_repo(repo if isinstance(repo, str) else None)
        if not repo_norm:
            skipped["non_github_repo"] += 1
            continue
        items.append(SearchItem(repo=repo_norm, path=path, html_url=""))
    return items, skipped


def skip_trivia(src: str, start: int = 0) -> int:
    idx = start
    length = len(src)
    while idx < length:
        ch = src[idx]
        if ch == "\ufeff":
            idx += 1
            continue
        if ch in " \t\r\n":
            idx += 1
            continue
        if ch != "#":
            return idx
        if idx + 1 < length and src[idx + 1] == "[":
            eq_idx = idx + 2
            while eq_idx < length and src[eq_idx] == "=":
                eq_idx += 1
            if eq_idx < length and src[eq_idx] == "[":
                close = "]" + ("=" * (eq_idx - (idx + 2))) + "]"
                end = src.find(close, eq_idx + 1)
                if end == -1:
                    return length
                idx = end + len(close)
                continue
        newline = src.find("\n", idx + 1)
        if newline == -1:
            return length
        idx = newline + 1
    return length


def looks_like_cmake(src: str) -> bool:
    idx = skip_trivia(src)
    if idx >= len(src):
        return True
    ch = src[idx]
    if not (ch.isalpha() or ch == "_"):
        return False
    end = idx + 1
    while end < len(src):
        if src[end].isalnum() or src[end] == "_":
            end += 1
            continue
        break
    end = skip_trivia(src, end)
    return end < len(src) and src[end] == "("


def normalize_repo_entry(entry: str) -> str:
    raw = entry.strip().strip("/")
    prefix = "https://github.com/"
    if raw.startswith(prefix):
        parts = raw.split("/")
        if len(parts) >= 5:
            return f"{parts[3]}/{parts[4]}"
    return raw


def normalize_path_entry(entry: str) -> str:
    raw = entry.strip()
    if not raw:
        return raw
    prefix = "https://github.com/"
    if raw.startswith(prefix):
        parts = raw.split("/")
        if len(parts) >= 7 and parts[5] == "blob":
            return f"{parts[3]}/{parts[4]}/" + "/".join(parts[6:])
    return raw.lstrip("/")


def load_skiplist(path: Path | None) -> tuple[set[str], set[str]]:
    if not path or not path.exists():
        return set(), set()
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise RuntimeError(f"Invalid TOML in {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise RuntimeError(f"Invalid skiplist in {path}: expected a TOML table")
    repos = {normalize_repo_entry(item) for item in ensure_list(data.get("repos"))}
    paths = {normalize_path_entry(item) for item in ensure_list(data.get("paths"))}
    repos.discard("")
    paths.discard("")
    return repos, paths


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
        "--backend",
        choices=["github", "sourcegraph"],
        default=cfg("search_backend", DEFAULT_CONFIG["search_backend"]),
        help="Search backend to use for corpus collection.",
    )
    parser.add_argument("--search-limit", type=int, default=None)
    parser.add_argument("--size-min", type=int, default=None)
    parser.add_argument("--size-max", type=int, default=None)
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
    parser.add_argument(
        "--exact-basename",
        dest="exact_basename",
        action="store_true",
        default=None,
        help="Only include paths whose basename is CMakeLists.txt (case-insensitive).",
    )
    parser.add_argument(
        "--allow-suffix",
        dest="exact_basename",
        action="store_false",
        help="Allow paths that merely contain CMakeLists.txt in the name.",
    )
    parser.add_argument(
        "--filter-invalid",
        dest="filter_invalid",
        action="store_true",
        default=None,
        help="Skip files that do not look like CMake based on a lightweight heuristic.",
    )
    parser.add_argument(
        "--no-filter-invalid",
        dest="filter_invalid",
        action="store_false",
        help="Do not skip files that fail the heuristic.",
    )
    parser.add_argument(
        "--skiplist",
        type=Path,
        default=None,
        help="TOML file listing repos/paths to skip.",
    )
    parser.add_argument("--cache-ttl", type=int, default=None)
    parser.add_argument("--no-cache", dest="cache_enabled", action="store_false")
    parser.add_argument("--cache", dest="cache_enabled", action="store_true")
    parser.add_argument(
        "--search-cache",
        type=Path,
        default=None,
        help="Path to store GitHub search results cache.",
    )
    args = parser.parse_args()

    base_dir = config_path.parent if config_path else None

    corpus_dir_cli = args.corpus_dir
    corpus_dir_cfg = normalize_optional_path(
        cfg("corpus_dir", cfg("artifacts_dir", None)),
        base_dir,
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
        workdir = normalize_optional_path(cfg("workdir", None), base_dir) or corpus_dir
    if workdir is None:
        workdir = default_corpus_dir()

    if args.out is not None:
        out_path = normalize_optional_path(args.out)
    elif corpus_dir_cli is not None:
        out_path = None
    else:
        out_path = normalize_optional_path(cfg("out", None), base_dir)
    if out_path is None:
        out_path = workdir / "files.txt"

    if args.license_cache is not None:
        license_cache_path = normalize_optional_path(args.license_cache)
    elif corpus_dir_cli is not None:
        license_cache_path = None
    else:
        license_cache_path = normalize_optional_path(cfg("license_cache", None), base_dir)
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
    if args.exact_basename is None:
        args.exact_basename = bool(
            cfg("exact_basename", DEFAULT_CONFIG["exact_basename"])
        )
    if args.filter_invalid is None:
        args.filter_invalid = bool(
            cfg("filter_invalid", DEFAULT_CONFIG["filter_invalid"])
        )
    if args.cache_enabled is None:
        args.cache_enabled = bool(
            cfg("cache_enabled", DEFAULT_CONFIG["cache_enabled"])
        )
    if args.cache_ttl is None:
        args.cache_ttl = normalize_optional_int(
            cfg("cache_ttl_seconds", DEFAULT_CONFIG["cache_ttl_seconds"])
        )
    if args.cache_ttl is None:
        args.cache_ttl = DEFAULT_CONFIG["cache_ttl_seconds"]
    if args.search_limit is None:
        args.search_limit = normalize_optional_int(cfg("search_limit", None))
    if args.search_limit is None:
        args.search_limit = DEFAULT_CONFIG["search_limit"]
    skiplist_path = args.skiplist
    if skiplist_path is None:
        skiplist_path = normalize_optional_path(cfg("skiplist", None), base_dir)

    search_cache_path = args.search_cache
    if search_cache_path is None:
        search_cache_path = normalize_optional_path(cfg("search_cache", None), base_dir)

    size_min = normalize_optional_int(args.size_min)
    if size_min is None:
        size_min = normalize_optional_int(cfg("size_min", 0)) or 0
    size_max = normalize_optional_int(args.size_max)
    if size_max is None:
        size_max = normalize_optional_int(cfg("size_max", None))
    if size_min < 0:
        print("size_min must be >= 0", file=sys.stderr)
        return 2
    if size_max is not None and size_max < size_min:
        print("size_max must be >= size_min", file=sys.stderr)
        return 2
    if args.backend == "github":
        if size_max is None:
            size_max = resolve_size_max(size_min, args.count, args.query_extra)
    else:
        if size_min != 0 or size_max is not None:
            print(
                "Note: size filters are ignored for Sourcegraph backend.",
                file=sys.stderr,
            )

    global RATE_LIMIT_WAIT
    global RATE_LIMIT_SLEEP
    global SEARCH_CACHE_ENABLED
    global SEARCH_CACHE_TTL_SECONDS
    global SEARCH_CACHE_PATH
    global SEARCH_CACHE
    global SEARCH_CACHE_HITS
    global SEARCH_CACHE_MISSES
    RATE_LIMIT_WAIT = args.wait
    RATE_LIMIT_SLEEP = args.sleep
    SEARCH_CACHE_ENABLED = args.cache_enabled
    SEARCH_CACHE_TTL_SECONDS = args.cache_ttl
    if search_cache_path is None:
        SEARCH_CACHE_PATH = workdir / "search_cache.json"
    else:
        SEARCH_CACHE_PATH = normalize_optional_path(search_cache_path, base_dir)
    SEARCH_CACHE = {}
    SEARCH_CACHE_HITS = 0
    SEARCH_CACHE_MISSES = 0

    if SEARCH_CACHE_ENABLED and SEARCH_CACHE_PATH and SEARCH_CACHE_PATH.exists():
        try:
            SEARCH_CACHE = json.loads(
                SEARCH_CACHE_PATH.read_text(encoding="utf-8")
            )
        except json.JSONDecodeError:
            SEARCH_CACHE = {}

    if args.count <= 0:
        print("count must be > 0", file=sys.stderr)
        return 2

    repos_dir = workdir / "repos"
    workdir.mkdir(parents=True, exist_ok=True)
    repos_dir.mkdir(parents=True, exist_ok=True)

    allow_licenses = set(args.allow_license)
    deny_licenses = set(args.deny_license)
    skipped_by_license: dict[str, int] = defaultdict(int)
    skipped_by_name: dict[str, int] = defaultdict(int)
    skipped_by_content: dict[str, int] = defaultdict(int)
    skipped_by_skiplist: dict[str, int] = defaultdict(int)
    skipped_by_backend: dict[str, int] = defaultdict(int)

    skip_repos, skip_paths = load_skiplist(skiplist_path)

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

    if args.backend == "github":
        fetch_items_for_size_range(
            size_min,
            size_max,
            args.query_extra,
            license_cache,
            allow_licenses,
            deny_licenses,
            args.skip_unknown_license,
            skipped_by_license,
            args.exact_basename,
            skipped_by_name,
            skip_repos,
            skip_paths,
            skipped_by_skiplist,
            selected,
            seen,
            per_repo,
            args.max_per_repo,
            args.count,
            args.per_page,
        )
    else:
        if not shutil.which("src"):
            print("Sourcegraph backend requires the `src` CLI on PATH.", file=sys.stderr)
            print("Install: https://docs.sourcegraph.com/cli", file=sys.stderr)
            return 2
        if args.allow_license or args.deny_license or args.skip_unknown_license:
            print(
                "Note: license filters are not enforced for Sourcegraph searches.",
                file=sys.stderr,
            )
        auto_limit = max(args.count * 2, args.count + 50)
        search_limit = args.search_limit if args.search_limit > 0 else min(auto_limit, 1000)
        query = sourcegraph_query(args.query_extra)
        items, backend_skipped = sourcegraph_search(query, search_limit)
        for key, value in backend_skipped.items():
            skipped_by_backend[key] += value
        for item in items:
            key = (item.repo, item.path)
            if key in seen:
                continue
            if args.exact_basename and not is_cmakelists_path(item.path):
                skipped_by_name["basename_mismatch"] += 1
                continue
            if item.repo in skip_repos:
                skipped_by_skiplist["repo"] += 1
                continue
            repo_path = f"{item.repo}/{item.path}"
            if repo_path in skip_paths:
                skipped_by_skiplist["path"] += 1
                continue
            if per_repo[item.repo] >= args.max_per_repo:
                continue
            selected.append(item)
            seen.add(key)
            per_repo[item.repo] += 1
            if len(selected) >= args.count:
                break

    if not selected:
        print(
            "No files found. Try adjusting size filters, --query-extra, or --search-limit.",
            file=sys.stderr,
        )
        return 1
    if args.backend == "sourcegraph" and len(selected) < args.count:
        print(
            f"Only collected {len(selected)} files from Sourcegraph. "
            "Increase --search-limit or loosen filters.",
            file=sys.stderr,
        )

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
                if args.filter_invalid:
                    src = read_text_lossy(file_path)
                    if not looks_like_cmake(src):
                        skipped_by_content["non_cmake"] += 1
                        continue
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
    if skipped_by_name:
        print("Skipped by name:")
        for name, count in sorted(skipped_by_name.items()):
            print(f"  {name}: {count}")
    if skipped_by_content:
        print("Skipped by content:")
        for name, count in sorted(skipped_by_content.items()):
            print(f"  {name}: {count}")
    if skipped_by_skiplist:
        print("Skipped by skiplist:")
        for name, count in sorted(skipped_by_skiplist.items()):
            print(f"  {name}: {count}")

    if SEARCH_CACHE_ENABLED and SEARCH_CACHE_PATH and args.backend == "github":
        SEARCH_CACHE_PATH.parent.mkdir(parents=True, exist_ok=True)
        SEARCH_CACHE_PATH.write_text(
            json.dumps(SEARCH_CACHE, indent=2), encoding="utf-8"
        )
        print(
            "Search cache: hits {}, misses {}, entries {}, path {}".format(
                SEARCH_CACHE_HITS,
                SEARCH_CACHE_MISSES,
                len(SEARCH_CACHE),
                SEARCH_CACHE_PATH,
            )
        )
    if skipped_by_backend:
        print("Skipped by backend:")
        for name, count in sorted(skipped_by_backend.items()):
            print(f"  {name}: {count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
