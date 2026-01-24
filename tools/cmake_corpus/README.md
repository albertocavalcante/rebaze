# CMake Corpus Collection

This tool collects a CMake file corpus from GitHub using the `gh` CLI and prepares
the files for parser testing via sparse checkouts.

## Quick start (100 files)

```bash
uv run tools/cmake_corpus/collect.py
```

This uses defaults from `tools/cmake_corpus/config.toml` if present.

This writes into the corpus directory:
- `files.txt` (file list for parsing)
- `manifest.jsonl` (repo/path metadata)
- `repos/` (sparse clones)

## Parser run

```bash
cargo run -p rebaze-cmake --bin corpus -- --list /path/to/corpus/files.txt --allow-fail
```

## Scale up (example: 10k files)

```bash
uv run tools/cmake_corpus/collect.py --count 10000 --max-per-repo 2
```

If you hit GitHub search rate limits, rerun with:

```bash
uv run tools/cmake_corpus/collect.py --wait
```

## License filters

By default, AGPL-3.0 variants are excluded. You can override this behavior:

```bash
uv run tools/cmake_corpus/collect.py --deny-license GPL-3.0-only --skip-unknown-license
```

To only allow specific SPDX IDs, repeat `--allow-license`:

```bash
uv run tools/cmake_corpus/collect.py --allow-license MIT --allow-license Apache-2.0
```

## Config file

`tools/cmake_corpus/config.toml` controls defaults like count, licenses, size ranges, and rate limit settings.
CLI flags override the config.

If no corpus directory is configured, the default is:
- `REBAZE_CMAKE_CORPUS_DIR` if set
- otherwise a repo-local `.rebaze/cmake-corpus` if a repo root is found
- otherwise `XDG_CACHE_HOME/rebaze/cmake-corpus` if set
- otherwise the OS temp directory + `rebaze-cmake-corpus`

If `size_max` is empty, the script auto-expands the size range until it can satisfy
the requested count (or no larger files exist).

By default, only exact `CMakeLists.txt` basenames are accepted (case-insensitive).
Use `--allow-suffix` to include files that merely contain `CMakeLists.txt` in the name.

The collector also applies a lightweight heuristic to skip files that do not look
like CMake (first non-trivia token must be a command name followed by `(`). Disable
with `--no-filter-invalid` if you need raw results.

Search results are cached locally to reduce GitHub API usage. Configure with:
- `cache_enabled`, `cache_ttl_seconds`, and `search_cache`
- CLI: `--no-cache`, `--cache-ttl`, `--search-cache`

Skiplist support:
- `tools/cmake_corpus/skiplist.toml` lets you skip known-bad repos/paths.
- Entries can be `owner/repo/path` or a GitHub file URL.
- Override with `--skiplist /path/to/skiplist.toml`.

To use a different config file:

```bash
uv run tools/cmake_corpus/collect.py --config /path/to/cmake-corpus.toml
```

To keep all outputs together, set `corpus_dir` (or pass `--corpus-dir`).
Leaving `workdir`, `out`, or `license_cache` empty keeps them under `corpus_dir`.

Environment overrides:
- `REBAZE_CMAKE_CORPUS_DIR` sets the base corpus directory
- `REBAZE_CMAKE_CORPUS_FILES` sets the default list file for the corpus runner

## Notes

- Default search is `filename:CMakeLists.txt` across GitHub.
- Use `--size-min`/`--size-max` (or config) to control file size ranges.
- Adjust `--query-extra` if you need more coverage.
- Use `--no-fetch` to only build the manifest without cloning repos.
