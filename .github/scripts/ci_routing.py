"""Conservative CI routing and inexpensive repository-document checks."""

from __future__ import annotations

import os
import re
import subprocess
import sys
from dataclasses import dataclass
from itertools import pairwise
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING
from urllib.parse import unquote, urlsplit

if TYPE_CHECKING:
    from markdown_it.token import Token

REPOSITORY_DOCS = frozenset(
    {
        "README.md",
        "AGENTS.md",
        "CONTRIBUTING.md",
        "REVIEW.md",
        "docs/README.md",
        "docs/architecture.md",
        "docs/guarded-application.md",
        "docs/engineering-roadmap.md",
        "docs/refactoring-plan.md",
    }
)
SITE_FILES = frozenset(
    {
        "docs/astro.config.mjs",
        "docs/tsconfig.json",
        "docs/package.json",
        "docs/bun.lock",
    }
)
# This published MDX is also include_str!/compile_data for Rust config tests.
BUILD_AND_SITE_FILES = frozenset({"docs/src/content/docs/reference/config.mdx"})
ROUTING_FILES = frozenset(
    {
        ".github/scripts/ci_routing.py",
        ".github/scripts/test_ci_routing.py",
        ".github/scripts/setup-bun.sh",
        ".github/scripts/download-tool.sh",
        ".github/workflows/ci.yml",
        ".github/workflows/docs.yml",
    }
)
SHA = re.compile(r"(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})\Z")


@dataclass(frozen=True)
class Route:
    build: bool
    site: bool
    reason: str


def repository_doc(path: str) -> bool:
    return path in REPOSITORY_DOCS or (
        path.startswith("docs/operations/") and path.endswith(".md")
    )


def classify(paths: list[str]) -> Route:
    build = False
    site = False
    for path in paths:
        if repository_doc(path):
            continue
        if path in BUILD_AND_SITE_FILES:
            build = True
            site = True
            continue
        if path in SITE_FILES or path.startswith(("docs/src/", "docs/public/")):
            site = True
            continue
        # Unknown paths are build inputs until explicitly reviewed otherwise.
        build = True
        if path in ROUTING_FILES or path.startswith("docs/"):
            site = True
    return Route(build, site, f"classified {len(paths)} changed paths")


def git(root: Path, *args: str) -> bytes:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        timeout=60,
    ).stdout


def route_revision(root: Path, event: str, base: str, head: str, force: str) -> Route:
    if force == "true" or event == "workflow_dispatch":
        return Route(True, True, "explicit full run")
    if force != "false" or event not in {"push", "pull_request", "nightly"}:
        return Route(True, True, "unknown event or force value; full verification")
    if not SHA.fullmatch(base) or not SHA.fullmatch(head) or set(base) == {"0"}:
        return Route(True, True, "missing comparison revision; full verification")
    try:
        if event == "pull_request":
            base = git(root, "merge-base", base, head).decode("ascii").strip()
        paths = git(
            root,
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "-z",
            base,
            head,
            "--",
        )
        # No rename detection: both the deleted old and added new path participate.
        return classify(
            [p.decode("utf-8", "surrogateescape") for p in paths.split(b"\0") if p]
        )
    except (subprocess.SubprocessError, UnicodeError):
        return Route(True, True, "comparison unavailable; full verification")


def local_destinations(tokens: list[Token], line: int = 1) -> list[tuple[int, str]]:
    destinations: list[tuple[int, str]] = []
    for token in tokens:
        location = token.map[0] + 1 if token.map else line
        attribute = (
            "href"
            if token.type == "link_open"
            else "src"
            if token.type == "image"
            else None
        )
        if attribute and (target := token.attrGet(attribute)) is not None:
            if not isinstance(target, str):
                raise ValueError("Markdown destination must be text")
            destinations.append((location, target))
        if token.children:
            destinations.extend(local_destinations(token.children, location))
    return destinations


def check_docs(root: Path) -> None:
    """Check CommonMark/table links and top-level repository heading structure.

    The parser distinguishes code/literal text from actual links and images.
    Published MDX, URL reachability, and anchor validation remain separate checks.
    """
    # Only the docs command/tests need these small, hash-pinned CI dependencies.
    from markdown_it import MarkdownIt

    parser = MarkdownIt("commonmark").enable("table")
    paths = git(root, "ls-files", "-z").split(b"\0")
    errors: list[str] = []
    for raw in paths:
        path = raw.decode("utf-8", "surrogateescape")
        if not repository_doc(path):
            continue
        file = root / path
        if file.is_symlink() or not file.resolve().is_relative_to(root.resolve()):
            errors.append(
                f"{path}: repository documentation must not escape through symlinks"
            )
            continue
        tokens = parser.parse(file.read_text(encoding="utf-8"))
        headings = [
            int(token.tag[1:])
            for token in tokens
            if token.type == "heading_open" and token.level == 0
        ]
        for number, target in local_destinations(tokens):
            parsed = urlsplit(target)
            if parsed.scheme or parsed.netloc or not parsed.path:
                continue
            destination = file.parent / unquote(parsed.path)
            if (
                PurePosixPath(unquote(parsed.path)).is_absolute()
                or not destination.resolve().is_relative_to(root.resolve())
                or not destination.exists()
            ):
                errors.append(
                    f"{path}:{number}: missing or escaping local destination {target!r}"
                )
        if headings.count(1) != 1 or any(b > a + 1 for a, b in pairwise(headings)):
            errors.append(
                f"{path}: require one top-level H1 and no skipped heading levels"
            )
    if errors:
        raise ValueError("\n".join(errors))


def check_gate(env: dict[str, str]) -> None:
    if env.get("ROUTING_EVENT") not in {"push", "pull_request", "workflow_dispatch"}:
        raise ValueError("unknown gate event")
    if env.get("ROUTING_RESULT") != "success":
        raise ValueError("change routing or lightweight checks failed/cancelled")
    build = env.get("BUILD_REQUIRED")
    site = env.get("SITE_REQUIRED")
    if build not in {"true", "false"} or site not in {"true", "false"}:
        raise ValueError("missing or invalid routing outputs")
    expected = {
        "CARGO_RESULT": "success" if build == "true" else "skipped",
        "BAZEL_RESULT": "success" if build == "true" else "skipped",
        "SITE_RESULT": "success"
        if site == "true" and env.get("ROUTING_EVENT") != "push"
        else "skipped",
    }
    for job, result in expected.items():
        if env.get(job) != result:
            raise ValueError(f"{job}: expected {result}, got {env.get(job)!r}")


def main() -> None:
    root = Path.cwd()
    command = sys.argv[1]
    if command == "classify":
        route = route_revision(
            root,
            os.environ.get("ROUTING_EVENT", ""),
            os.environ.get("BASE_SHA", ""),
            os.environ.get("HEAD_SHA", ""),
            os.environ.get("FORCE_FULL", "false"),
        )
        output = f"build={str(route.build).lower()}\nsite={str(route.site).lower()}\n"
        if destination := os.environ.get("GITHUB_OUTPUT"):
            with Path(destination).open("a", encoding="utf-8") as stream:
                _ = stream.write(output)
        print(output + route.reason)
    elif command == "check-docs":
        check_docs(root)
        print("Repository Markdown structure and local inline destinations passed.")
    elif command == "gate":
        check_gate(dict(os.environ))
        print("All selected checks passed; irrelevant jobs were deliberately skipped.")
    else:
        raise ValueError(f"unknown command: {command}")


if __name__ == "__main__":
    main()
