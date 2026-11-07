# Reviewing rebaze changes

Review the complete final diff and its behavior before merging a pull request.
Use this guide with [CONTRIBUTING.md](CONTRIBUTING.md); the author supplies the
checks and reproduction, and an independent reviewer examines their adequacy.

## Establish scope

Read the PR's problem statement, base/head commits, complete diff, and relevant
callers and tests. Trace changes across Cargo and Bazel targets. Check the
user-visible behavior against documentation and supported-platform claims.

Agent-created PRs require at least one independent reviewer agent. If one is
unavailable, record that as a readiness gap. Neither author self-review nor
passing CI replaces independent review.

## Inspect the boundaries that change

| Area                  | Questions to resolve                                                                                                                         |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Parsers/configuration | What input is trusted? Are unsupported forms, malformed input, recursion, and size limits handled explicitly?                                |
| Generation            | Do values remain data? Are binding/call names and labels valid? Are loads, extension imports, aliases, and dependency strategies consistent? |
| Filesystem writes     | Can a path escape the project or follow a symlink? What happens on a failed or interrupted write? Can existing files be recovered?           |
| Processes/network     | Which commands or endpoints execute? Are arguments, timeouts, status failures, credentials, and retries handled correctly?                   |
| Dependencies          | Do Cargo/Bazel declarations and locks agree? Do artifact URLs, versions, and verified hashes match?                                          |
| Compatibility         | Which Bazel/rule versions, languages, platforms, and fixtures actually ran? Is syntax being mistaken for build correctness?                  |
| CI/release            | Are permissions minimal, actions SHA-pinned with verified tags, and required checks intact? Is rollout/rollback executable?                  |
| Documentation         | Can the stated procedure be followed from a clean checkout? Does it describe current behavior and the right audience?                        |

Follow relevant failure and rollback paths. For example, native generation
finishes before writing, but an I/O failure during a multi-file write can leave
an updated subset. Post-generation Bazel failure does not restore original files.
A review must not claim transaction-wide rollback from per-file atomic replacement.

## Evaluate validation

- Verify that regressions fail on the base, or inspect a recorded base/head
  reproduction when the new test cannot compile there.
- Check assertions against observable behavior, including negative paths.
- Identify external fixtures that did not run. Missing corpus inputs and
  cross-compilation are different from a native end-to-end pass.
- Use the contributing guide's check matrix for the affected files. Required CI
  failures, unavailable checks, and unresolved findings remain visible blockers.
- Match review depth to risk. A documentation-only change needs command, link,
  behavior, and policy review; it does not require invented unit tests.

## Resolve findings and record the result

Give each actionable finding a precise location, trigger, consequence, and
suggested direction. Address it before merge. After a fix or other head change,
obtain follow-up independent review of the changed code and relevant paths.

Keep detailed evidence in an expandable PR section:

- Base and final reviewed head SHA; reviewer identity and review scope.
- Findings, fixes, and follow-up verdict.
- Commands, CI links, fixture/platform coverage, and failed or missing checks.
- Compatibility limits, rollout steps, and rollback behavior.

Keep the visible PR description focused on why the change matters and its
validation. Use headings, short bullets, and compact tables where useful.

## Merge gate

The final reviewed head must match the PR head. Required checks must pass, and
all actionable findings and review threads must be resolved. Preserve unrelated
work, secrets, and repository visibility.

Use a squash merge with a handcrafted Conventional Commit subject of at most
72 characters and a concise body. Never create merge commits, including on
feature branches. Preserve work before rewriting published branches and use an
explicit force-with-lease. Verify the PR is merged and record the target commit.

The owner's standing authorization permits agents to land a ready change;
an explicit request to keep the PR open takes precedence. Missing infrastructure
or review evidence is not permission to bypass an enforced check.
