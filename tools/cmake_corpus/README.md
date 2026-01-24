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

`tools/cmake_corpus/config.toml` controls defaults like count, licenses, and rate limit settings.
CLI flags override the config.

If no corpus directory is configured, the default is:
- `REBAZE_CMAKE_CORPUS_DIR` if set
- otherwise a repo-local `.rebaze/cmake-corpus` if a repo root is found
- otherwise `XDG_CACHE_HOME/rebaze/cmake-corpus` if set
- otherwise the OS temp directory + `rebaze-cmake-corpus`

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
- Adjust `--start-year` or `--query-extra` if you need more coverage.
- Use `--no-fetch` to only build the manifest without cloning repos.
