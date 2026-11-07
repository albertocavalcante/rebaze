# Documentation guide

Choose the document's audience and task before adding a page. Keep each procedure
in one authoritative place, then link to it from other entry points.

## What belongs where

| Document                       | Audience and content                                                                                        |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| Root `README.md`               | New users: purpose, honest support status, one preview-first usage path, build entry point, and links       |
| `CONTRIBUTING.md`              | Contributors: prerequisites, setup, actual CI commands, code/test conventions, and PR workflow              |
| `REVIEW.md`                    | Authors/reviewers: complete-diff review, failure and rollback questions, required evidence, and merge gates |
| `AGENTS.md`                    | Coding agents: repository-specific instructions and links to contributor/review policy                      |
| `docs/architecture.md`         | Maintainers: current crate responsibilities, data flow, API/effect boundaries, and limitations              |
| `docs/engineering-roadmap.md`  | Maintainers: the next milestone, dependency order, completion evidence, and deferred work                   |
| `docs/operations/`             | Operators: runner/release procedures, prerequisites, verification, failure recovery, and rollback           |
| `docs/src/content/docs/`       | Users: published tutorials, task guides, conceptual explanations, and reference                             |
| `tools/cmake_corpus/README.md` | Corpus users: explicit external collection/execution procedures and report limits                           |
| `docs/refactoring-plan.md`     | Historical proposal: earlier file-decomposition ideas; not the current roadmap                              |

Do not copy the full contributing checklist into the README, repeat a CLI flag
table across guides, or put live credentials/host configuration into a runbook.
Add an architecture decision record only when a consequential choice needs its
own rationale and alternatives. Use release notes for shipped user-visible
changes; use the roadmap for work that has not shipped.

Repository Markdown outside `src/content/docs/` is browsed in GitHub and is not
published automatically. Link to it through its repository URL from site pages.
Use relative links between repository Markdown files.

## Work on the site

The user site uses Astro Starlight. Run these commands from the repository root:

```bash
cd docs
bun install --frozen-lockfile
bun run dev
```

Open the local `/rebaze/` path printed by the development server. Build and preview
with the package scripts, rather than Bun's standalone bundler command:

```bash
bun run build
bun run preview
```

`bun run build` validates MDX and internal site links, produces a search index,
and writes `docs/dist`. It does not check repository Markdown links or prove that
CLI examples build a migrated project. The lockfile pins packages; CI installs checksum-verified Bun from
`.github/scripts/setup-bun.sh`.

## Choose a page type

| Site section       | Reader's task                                 | Example                          |
| ------------------ | --------------------------------------------- | -------------------------------- |
| `getting-started/` | Install and try the smallest supported path   | Source build and preview         |
| `tutorials/`       | Learn through a complete reproducible example | Migrate a small CMake project    |
| `guides/`          | Complete a specific task                      | Read CMake File API replies      |
| `concepts/`        | Understand behavior and tradeoffs             | Dependency strategies            |
| `reference/`       | Look up exact syntax and behavior             | CLI flags and configuration keys |

Use one primary purpose per page. A tutorial needs inputs, commands, expected
results, and a way to inspect failures. Reference describes implemented behavior;
it must distinguish reserved fields, experimental paths, and unsupported cases.

## Add or change a page

1. Create an `.mdx` file under the appropriate `src/content/docs/` directory.
2. Start with `title` and `description` frontmatter, followed by a short purpose
   statement. Use `##` for content headings; Starlight supplies the page title.
3. Add a sidebar entry in `astro.config.mjs`. Link related pages with their
   `/rebaze/.../` site paths.
4. Use language-tagged code blocks, marked placeholders, and expected output
   where it helps verify the result. Keep commands and versions tied to code or
   executable evidence.
5. Run `bun run build`, inspect the affected page, and check repository links too.

MDX treats angle brackets and braces as syntax. Use Markdown links, avoid HTML
comments, and import any Starlight components you use. Keep claims proportional
to evidence: syntax acceptance, successful generation, compilation, and passing
tests are separate outcomes.

## Documentation in a pull request

State what changed for the reader and which commands/links you checked. Update
CLI/configuration reference with behavior changes and adjust tutorials when
output or prerequisites change. Put implementation details and remaining gates
in architecture/roadmap documents rather than the user's first-run path.

[CONTRIBUTING.md](../CONTRIBUTING.md) and [REVIEW.md](../REVIEW.md) define validation
and review requirements. The [documentation workflow](../.github/workflows/docs.yml)
builds/deploys the site on matching pushes to `main` and supports manual dispatch.
Repository-only docs do not trigger deployment. CI builds the site for PR changes
to published content, assets, configuration, or dependencies; it does not deploy
PR content. Continue validating site changes locally with the commands above.
