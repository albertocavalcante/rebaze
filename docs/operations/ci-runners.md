# CI runner operations

Rebaze uses the dedicated `air-rebaze` runmux pool for Linux ARM64 after its
activation gates pass. The `CI_LINUX_RUNNER` repository variable selects the
pool; deleting it restores the workflow's hosted `ubuntu-24.04` fallback.
Release matrix jobs retain their configured platforms. ARM64 results do not
establish native x86-64, macOS, or Windows behavior.

## Identify the service you are changing

Read the primary LaunchAgent's `ProgramArguments` before using an onboarding
helper. Its `--config` path can differ from the CLI default `config.toml`.
`runmux status` can reach the active daemon through a shared control socket while
an unqualified `runmux doctor` checks a different file.

On the owning host, inspect the exact service and pass its active config to
subsequent checks:

```bash
plutil -extract ProgramArguments json -o - \
  "$HOME/Library/LaunchAgents/engineer.alberto.runmux.plist"
```

Use the explicit service label when multiple runmux LaunchAgents exist. Preserve
other profiles, scopes, operator pauses, and services. Do not replace a restricted
active profile with an older broad config just to make default commands agree.
Private configs must be regular files owned by the operator with mode `0600`.

## Grant access without exposing credentials

1. Inspect the current App selection and back up both managed grant specs
   privately. Add only `rebaze`; preserve every existing repository and App.
2. Require an access plan with exactly `+ rebaze` and zero removals. Apply with
   the trusted `runmux-app-access` wrapper, then require a clean follow-up plan.
3. If Keychain value retrieval fails over SSH with exit 36, distinguish execution
   context from a missing/expired credential. Metadata lookup alone does not prove
   secret access. A bounded, one-shot LaunchAgent in the owning GUI user session
   can run the same trusted wrapper with minimal PATH/HOME and private logs.
   Inspect its exit status and remove that temporary job afterward. Do not export
   the credential, print it, put it in a file, or change Keychain access controls.
4. If the GUI session also cannot use the credential, resolve that authentication
   failure interactively on the Air. Missing or expired credentials require the
   wrapper's setup/renew procedure; do not substitute a broader token.

The verified onboarding added one grant and preserved 59 others. Access is still
limited to selected repositories; the App was not widened to all repositories.

## Add the pool and restart only when idle

The active `air-ci` pool disallows public repositories. Rebaze therefore uses its
own `air-rebaze` pool with one explicit scope/allow-list entry and
`allow_public = true`. Existing private-pool policies stay unchanged. Pool names
must retain the host's `air-` prefix.

For an existing compatible pool, the guarded onboarding helper can add its scope
and allow-list entry. Adding a dedicated pool requires a validated profile update:

1. Build a private candidate and prove its parsed difference is only the intended
   new pool. Keep the image digest, isolation settings, resource limits, other
   pools, host settings, and App configuration unchanged.
2. Run doctor against that exact candidate. Check both **host state-volume free
   space** and **engine filesystem free space**. A sparse VM disk can leave the
   host below its gate while the engine reports ample capacity. Never lower a
   gate to force onboarding or prune unrelated images/builds. An ownership-scoped
   GC plan with zero candidates is not permission for a global prune.
3. Snapshot operator pauses, pause admission only on the primary daemon's pools,
   and wait for zero live slots and assignments. Never terminate a running job.
4. Recheck doctor after draining, then compare the active config and service PID
   with the snapshot. Back up and atomically replace the config only if they still
   match. Restart that exact service and verify every expected scope/session.
5. Resume only pools paused by this operation. A newly added pool starts unpaused;
   keep workflow dispatch/routing disabled until its health checks finish. Do not
   claim that an existing-pool pause automatically covers a new pool.

A pre-replacement failure resumes only this operation's pauses. A failure after
replacement requires inspecting the actual config/service/pause state before
recovery; do not blindly rerun the operation or overwrite concurrent work.

## Verify execution before routing CI

Push the reviewed workflow change, then dispatch the canary on that exact ref:

```bash
gh workflow run runmux-canary.yml --repo albertocavalcante/rebaze --ref REVIEWED_REF
```

Require Linux, ARM64, a nonzero UID, an `rmx-air-` runner name, and executable
runner temporary storage. The canary records runner identity and commit in its
job summary. Record its run URL, head SHA, and actual runner identity from the
Actions job metadata too. A queue or a successful dispatch is not execution proof.

After it passes, enable the repository route:

```bash
gh variable set CI_LINUX_RUNNER --repo albertocavalcante/rebaze --body air-rebaze
```

Run both complete CI jobs on the intended head and record their actual runners.
The CI workflow supports manual dispatch after this change is on the default
branch. Before merge, a new PR run can exercise its reviewed workflow. A canary
alone does not satisfy the build/test gate.

The minimal image does not include Rust or Bazel. Repository bootstrap scripts
verify architecture-specific SHA-256 pins before execution, then install the
compiler/components from `rust-toolchain.toml` and Bazel from `.bazelversion`.
Bazel's version/platform must have a committed checksum entry. Do not enable the
cache action's unverified executable downloader. Offline bootstrap regressions
run in CI, including corrupt bytes, failed downloads, unsupported architectures,
and the existing-rustup path.

The pool has two CPUs and 4 GiB RAM. CI bounds compilation to two jobs and disables
Cargo development debug information to reduce memory/disk pressure. When tool
homes are unset, Rust setup defaults them to disk-backed `RUNNER_TEMP`; inherited
explicit homes are preserved. Avoid the image's small/noexec `/tmp` for downloaded
executables. Setup publishes environment changes only after success.

## Public pull requests and failure recovery

GitHub's effective fork approval policy is `all_external_contributors`. The pool
allow-list limits base repositories; it is **not** a fork-contributor filter.
Review a fork's code and workflow changes before approving execution. Workers are
ephemeral and non-root, with isolation checked by runmux; App credentials remain
on the host. Workflow token permissions stay read-only and checkout credentials
are not persisted. Privileged PR/workflow-run events remain denied by the pool.

If jobs queue, check App grant, explicit pool scope/policy, label, host/engine
headroom, operator pauses, capacity, service state, and concurrency independently.
A running service or successful doctor alone does not prove workload compatibility.
After a reboot, verify the user session, engine, daemon and scope health again;
do not treat a saved plist as a running service.

Restore hosted routing for future job evaluation with:

```bash
gh variable delete CI_LINUX_RUNNER --repo albertocavalcante/rebaze
```

Inspect already queued/running jobs separately. Retain the private config backup;
restore it only after draining and checking for intervening changes. Keep App
changes add-only during onboarding. Record activation and recovery evidence in
the PR or operational journal. The [roadmap](../engineering-roadmap.md) tracks
remaining platform and delivery work.
