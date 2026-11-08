"""Behavioral tests for conservative diffs, job results and Markdown checks."""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Protocol, cast, override
from unittest.mock import patch

import ci_routing as routing
from ruamel.yaml import YAML


class ClassificationTests(unittest.TestCase):
    def test_repository_docs_do_not_build_or_deploy(self) -> None:
        for path in [
            "README.md",
            "CONTRIBUTING.md",
            "REVIEW.md",
            "AGENTS.md",
            "docs/engineering-roadmap.md",
            "docs/architecture.md",
            "docs/guarded-application.md",
            "docs/operations/ci-runners.md",
        ]:
            with self.subTest(path=path):
                route = routing.classify([path])
                self.assertFalse(route.build)
                self.assertFalse(route.site)

    def test_site_inputs_build_only_the_site(self) -> None:
        for path in [
            "docs/src/content/docs/index.mdx",
            "docs/src/components/Head.astro",
            "docs/src/styles/global.css",
            "docs/public/favicon.svg",
            "docs/astro.config.mjs",
            "docs/package.json",
            "docs/bun.lock",
            "docs/tsconfig.json",
        ]:
            with self.subTest(path=path):
                route = routing.classify([path])
                self.assertFalse(route.build)
                self.assertTrue(route.site)

    def test_config_reference_also_checks_its_compiled_examples(self) -> None:
        route = routing.classify(["docs/src/content/docs/reference/config.mdx"])
        self.assertTrue(route.build)
        self.assertTrue(route.site)

    def test_build_inputs_and_unknown_files_never_skip_builds(self) -> None:
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "MODULE.bazel.lock",
            "BUILD.bazel",
            "crates/a/src/lib.rs",
            "crates/a/tests/fixtures/input.md",
            "crates/a/src/templates/helper.bzl",
            "new-tool-config",
            "new\nfile.md",
            ".github/workflows/nightly.yml",
            ".github/scripts/setup-rust.sh",
        ]:
            with self.subTest(path=path):
                self.assertTrue(routing.classify([path]).build)
        route = routing.classify(["docs/engineering-roadmap.md", "crates/a/src/lib.rs"])
        self.assertTrue(route.build)
        self.assertFalse(route.site)
        self.assertTrue(routing.classify([".github/workflows/docs.yml"]).site)
        self.assertTrue(routing.classify(["docs/unknown-config"]).build)
        self.assertFalse(routing.classify([]).build)

    def test_large_changes_do_not_hide_a_late_code_path(self) -> None:
        paths = [f"docs/src/content/docs/page-{i}.mdx" for i in range(4000)]
        paths.append("crates/a/src/lib.rs")
        self.assertTrue(routing.classify(paths).build)


class GitTests(unittest.TestCase):
    _root: Path | None = None
    _base: str | None = None

    @property
    def root(self) -> Path:
        assert self._root is not None, "Git fixture has not been set up"
        return self._root

    @property
    def base(self) -> str:
        assert self._base is not None, "Git fixture has not been set up"
        return self._base

    @override
    def setUp(self) -> None:
        fixture = tempfile.TemporaryDirectory()
        self.addCleanup(fixture.cleanup)
        self._root = Path(fixture.name)
        _ = self.git("init", "--initial-branch=main")
        _ = self.git("config", "user.name", "Routing tests")
        _ = self.git("config", "user.email", "routing@example.invalid")
        _ = self.git("config", "commit.gpgsign", "false")
        _ = self.git("config", "core.hooksPath", "/dev/null")
        self._base = self.commit("README.md", "# Test\n")

    def git(self, *args: str) -> str:
        return routing.git(self.root, *args).decode().strip()

    def commit(self, path: str, content: str) -> str:
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        _ = file.write_text(content)
        _ = self.git("add", "--", path)
        _ = self.git("commit", "-m", "fixture")
        return self.git("rev-parse", "HEAD")

    def route(
        self, head: str, event: str = "push", base: str | None = None
    ) -> routing.Route:
        return routing.route_revision(
            self.root, event, base or self.base, head, "false"
        )

    def test_multi_commit_push_and_nightly_include_earlier_code(self) -> None:
        _ = self.commit("src/main.rs", "fn main() {}\n")
        head = self.commit("README.md", "# Updated\n")
        self.assertTrue(self.route(head).build)
        self.assertTrue(self.route(head, "nightly").build)

    def test_rename_from_code_to_docs_cannot_hide_deletion(self) -> None:
        base = self.commit("input.rs", "source\n")
        (self.root / "docs").mkdir()
        _ = self.git("mv", "input.rs", "docs/architecture.md")
        _ = self.git("commit", "-m", "rename")
        self.assertTrue(self.route(self.git("rev-parse", "HEAD"), base=base).build)

    def test_delete_code_and_unusual_filename(self) -> None:
        base = self.commit("input\nname.rs", "source\n")
        _ = self.git("rm", "--", "input\nname.rs")
        _ = self.git("commit", "-m", "delete")
        self.assertTrue(self.route(self.git("rev-parse", "HEAD"), base=base).build)

    def test_pr_uses_merge_base_not_unrelated_base_branch_changes(self) -> None:
        _ = self.git("switch", "-c", "topic")
        head = self.commit("docs/architecture.md", "# Architecture\n")
        _ = self.git("switch", "main")
        newer_base = self.commit("base-code.rs", "source\n")
        route = self.route(head, "pull_request", newer_base)
        self.assertFalse(route.build)
        self.assertFalse(route.site)

    def test_missing_history_invalid_event_and_manual_force_are_full(self) -> None:
        for base in ["", "0" * 40, "f" * 40, "--output=bad"]:
            self.assertTrue(
                routing.route_revision(
                    self.root, "push", base, self.base, "false"
                ).build
            )
        for event in ["unknown", "workflow_dispatch"]:
            self.assertTrue(self.route(self.base, event).build)
        for force in ["true", "invalid"]:
            route = routing.route_revision(
                self.root, "push", self.base, self.base, force
            )
            self.assertTrue(route.build)
            self.assertTrue(route.site)
        with patch("ci_routing.git", side_effect=subprocess.TimeoutExpired("git", 60)):
            self.assertTrue(self.route(self.base).build)

    def test_docs_only_range_and_empty_range_do_not_build(self) -> None:
        head = self.commit("docs/engineering-roadmap.md", "# Plan\n")
        self.assertFalse(self.route(head).build)
        self.assertFalse(self.route(head, "nightly").build)
        self.assertFalse(self.route(self.base).build)

    def test_protocol_guide_is_lightweight_but_code_mix_still_builds(self) -> None:
        head = self.commit("docs/guarded-application.md", "# Guarded application\n")
        route = self.route(head)
        self.assertFalse(route.build)
        self.assertFalse(route.site)
        head = self.commit("src/main.rs", "fn main() {}\n")
        route = self.route(head)
        self.assertTrue(route.build)
        self.assertFalse(route.site)

    def test_protocol_guide_links_are_checked(self) -> None:
        _ = self.commit(
            "docs/guarded-application.md", "# Guide\n\n[Broken](missing.md)\n"
        )
        with self.assertRaisesRegex(ValueError, "missing or escaping"):
            routing.check_docs(self.root)

    def test_document_checker_detects_broken_links_and_structure(self) -> None:
        _ = self.commit("docs/README.md", "# Docs\n\n[Root](../README.md)\n")
        routing.check_docs(self.root)
        _ = self.commit("docs/README.md", "# Docs\n\n[Missing](missing.md)\n")
        with self.assertRaisesRegex(ValueError, "missing or escaping"):
            routing.check_docs(self.root)
        _ = self.commit("docs/README.md", "# Docs\n\n### Skipped\n")
        with self.assertRaisesRegex(ValueError, "heading levels"):
            routing.check_docs(self.root)

    def test_document_checker_ignores_fenced_examples_and_external_urls(self) -> None:
        _ = self.commit(
            "README.md",
            "# Test\n\n[Web](https://example.invalid)\n\n```md\n[Example](missing.md)\n```\n",
        )
        routing.check_docs(self.root)
        _ = self.commit("README.md", "# Test\n[Escape](../outside.md)\n")
        with self.assertRaisesRegex(ValueError, "escaping"):
            routing.check_docs(self.root)

    def test_literal_markdown_examples_are_not_links(self) -> None:
        examples = [
            "Write `[caption](missing.md)` to demonstrate link syntax.",
            r"Write \[caption](missing.md) to demonstrate escaped text.",
            "> ```md\n> [caption](missing.md)\n> ```",
            "- Example:\n\n    ```md\n    [caption](missing.md)\n    ```",
        ]
        for example in examples:
            with self.subTest(example=example):
                _ = self.commit("README.md", "# Example\n\n" + example + "\n")
                routing.check_docs(self.root)

    def test_reference_and_image_destinations_are_checked(self) -> None:
        for source in [
            "# Example\n\n[caption][target]\n\n[target]: missing.md\n",
            "# Example\n\n![picture](missing.png)\n",
        ]:
            with self.subTest(source=source):
                _ = self.commit("README.md", source)
                with self.assertRaisesRegex(ValueError, "missing or escaping"):
                    routing.check_docs(self.root)


class _YamlDocumentLoader(Protocol):
    def load(self, stream: str) -> object: ...


class DeploymentFilterTests(unittest.TestCase):
    def test_repository_doc_registration_excludes_main_deployment(self) -> None:
        root = Path(__file__).resolve().parents[2]
        source = (root / ".github/workflows/docs.yml").read_text(encoding="utf-8")
        # Keep untyped parser data at the IO boundary; validate the small schema
        # consumed by this policy check rather than relying on YAML source layout.
        parser = cast(_YamlDocumentLoader, YAML(typ="safe", pure=True))
        document = parser.load(source)
        for field in ("on", "push", "paths"):
            assert isinstance(document, dict), f"expected mapping before {field}"
            document = cast(dict[object, object], document)[field]
        assert isinstance(document, list), "push.paths must be a sequence"
        patterns: list[str] = []
        for pattern in cast(list[object], document):
            assert isinstance(pattern, str), "push.paths entries must be strings"
            patterns.append(pattern)
        include_position = patterns.index("docs/**")
        for path in sorted(routing.REPOSITORY_DOCS):
            if path.startswith("docs/"):
                with self.subTest(path=path):
                    exclusion = f"!{path}"
                    self.assertIn(exclusion, patterns)
                    self.assertGreater(patterns.index(exclusion), include_position)
        for path in routing.BUILD_AND_SITE_FILES:
            self.assertNotIn(f"!{path}", patterns)


class GateTests(unittest.TestCase):
    @staticmethod
    def env(
        build: str = "false", site: str = "false", event: str = "pull_request"
    ) -> dict[str, str]:
        return {
            "ROUTING_RESULT": "success",
            "BUILD_REQUIRED": build,
            "SITE_REQUIRED": site,
            "ROUTING_EVENT": event,
            "CARGO_RESULT": "success" if build == "true" else "skipped",
            "BAZEL_RESULT": "success" if build == "true" else "skipped",
            "SITE_RESULT": "success"
            if site == "true" and event != "push"
            else "skipped",
        }

    def test_selected_jobs_must_pass_and_irrelevant_jobs_must_skip(self) -> None:
        for build in ["true", "false"]:
            for site in ["true", "false"]:
                for event in ["push", "pull_request", "workflow_dispatch"]:
                    routing.check_gate(self.env(build, site, event))

    def test_missing_failed_cancelled_or_unexpected_jobs_are_not_green(self) -> None:
        for field in ["ROUTING_RESULT", "CARGO_RESULT", "BAZEL_RESULT", "SITE_RESULT"]:
            for result in ["failure", "cancelled", "", "skipped"]:
                env = self.env("true", "true")
                env[field] = result
                with (
                    self.subTest(field=field, result=result),
                    self.assertRaises(ValueError),
                ):
                    routing.check_gate(env)
        env = self.env()
        env["CARGO_RESULT"] = "success"
        with self.assertRaises(ValueError):
            routing.check_gate(env)
        for field in ["BUILD_REQUIRED", "SITE_REQUIRED", "ROUTING_EVENT"]:
            env = self.env()
            env[field] = ""
            with self.assertRaises(ValueError):
                routing.check_gate(env)


if __name__ == "__main__":
    _ = unittest.main()
