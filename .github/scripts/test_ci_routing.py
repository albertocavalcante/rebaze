"""Behavioral tests for conservative diffs, job results and Markdown checks."""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import ci_routing as routing


class ClassificationTests(unittest.TestCase):
    def test_repository_docs_do_not_build_or_deploy(self) -> None:
        for path in [
            "README.md",
            "CONTRIBUTING.md",
            "REVIEW.md",
            "AGENTS.md",
            "docs/engineering-roadmap.md",
            "docs/architecture.md",
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
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "--initial-branch=main")
        self.git("config", "user.name", "Routing tests")
        self.git("config", "user.email", "routing@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", "/dev/null")
        self.base = self.commit("README.md", "# Test\n")

    def git(self, *args: str) -> str:
        return routing.git(self.root, *args).decode().strip()

    def commit(self, path: str, content: str) -> str:
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(content)
        self.git("add", "--", path)
        self.git("commit", "-m", "fixture")
        return self.git("rev-parse", "HEAD")

    def route(
        self, head: str, event: str = "push", base: str | None = None
    ) -> routing.Route:
        return routing.route_revision(
            self.root, event, base or self.base, head, "false"
        )

    def test_multi_commit_push_and_nightly_include_earlier_code(self) -> None:
        self.commit("src/main.rs", "fn main() {}\n")
        head = self.commit("README.md", "# Updated\n")
        self.assertTrue(self.route(head).build)
        self.assertTrue(self.route(head, "nightly").build)

    def test_rename_from_code_to_docs_cannot_hide_deletion(self) -> None:
        base = self.commit("input.rs", "source\n")
        (self.root / "docs").mkdir()
        self.git("mv", "input.rs", "docs/architecture.md")
        self.git("commit", "-m", "rename")
        self.assertTrue(self.route(self.git("rev-parse", "HEAD"), base=base).build)

    def test_delete_code_and_unusual_filename(self) -> None:
        base = self.commit("input\nname.rs", "source\n")
        self.git("rm", "--", "input\nname.rs")
        self.git("commit", "-m", "delete")
        self.assertTrue(self.route(self.git("rev-parse", "HEAD"), base=base).build)

    def test_pr_uses_merge_base_not_unrelated_base_branch_changes(self) -> None:
        self.git("switch", "-c", "topic")
        head = self.commit("docs/architecture.md", "# Architecture\n")
        self.git("switch", "main")
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

    def test_document_checker_detects_broken_links_and_structure(self) -> None:
        self.commit("docs/README.md", "# Docs\n\n[Root](../README.md)\n")
        routing.check_docs(self.root)
        self.commit("docs/README.md", "# Docs\n\n[Missing](missing.md)\n")
        with self.assertRaisesRegex(ValueError, "missing or escaping"):
            routing.check_docs(self.root)
        self.commit("docs/README.md", "# Docs\n\n### Skipped\n")
        with self.assertRaisesRegex(ValueError, "heading levels"):
            routing.check_docs(self.root)

    def test_document_checker_ignores_fenced_examples_and_external_urls(self) -> None:
        self.commit(
            "README.md",
            "# Test\n\n[Web](https://example.invalid)\n\n```md\n[Example](missing.md)\n```\n",
        )
        routing.check_docs(self.root)
        self.commit("README.md", "# Test\n[Escape](../outside.md)\n")
        with self.assertRaisesRegex(ValueError, "escaping"):
            routing.check_docs(self.root)


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
    unittest.main()
