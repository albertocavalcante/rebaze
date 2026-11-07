# CI runner operations

Use this runbook to activate runmux only after its access, host, and workload
checks pass. Contributors can use hosted CI without configuring the Air.

## Routing and scope

The Linux jobs in `.github/workflows/ci.yml` use the `CI_LINUX_RUNNER` repository
variable, with `ubuntu-24.04` as the fallback. `air-ci` selects the runmux pool.
The manual `runmux-canary.yml` workflow selects `air-ci` directly.

Runmux supplies Linux ARM64. A green run there does not establish native x86-64,
macOS, or Windows behavior. Release matrix jobs retain their configured platforms.

## Activation gates

1. Verify the Air is connected and its configured container engine and runmux
   service are healthy. Inspect active jobs before changing service state.
2. Add only `rebaze` to the runmux App's selected-repository grant. The access
   plan must add that repository and remove none. Preserve all existing grants.
3. Add `albertocavalcante/rebaze` to the `air-ci` pool's scopes and
   `policy.allow_repos`. Use the host's guarded onboarding helper: validate a
   candidate, pause admission, drain existing work, back up, replace, and restart.
   Do not terminate running jobs to complete onboarding.
4. Dispatch **Runmux Canary** from the Actions tab, or run:

   ```bash
   gh workflow run runmux-canary.yml --repo albertocavalcante/rebaze --ref main
   ```

   Require Linux, ARM64, a nonzero UID, and a runner name starting `rmx-air-`.
   Record the run URL, head SHA, and actual runner identity.
5. After the canary passes, set the repository variable:

   ```bash
   gh variable set CI_LINUX_RUNNER --repo albertocavalcante/rebaze --body air-ci
   ```

   Run the complete CI workflow on the intended head, using a new push/PR or a
   deliberate rerun. CI currently has no `workflow_dispatch` trigger. Require both
   Bazel and Cargo jobs to pass and record their actual runner identities.

A canary alone does not satisfy the build/test gate. Verify the App grant, pool
policy, available runner capacity, and event permissions independently when a
job remains queued. Never widen the App to all repositories or print credentials.

## Failure and rollback

If a grant command cannot access its Keychain credential from SSH, resolve
credential access interactively on the Air. Do not substitute a broader token or
expose its value. A stopped engine, unavailable credential, or failed doctor is
a readiness gap; leave hosted routing available while addressing it.

Restore default hosted Linux routing with:

```bash
gh variable delete CI_LINUX_RUNNER --repo albertocavalcante/rebaze
```

This changes routing for future job evaluation; inspect already queued/running
jobs separately. Keep pool/App changes add-only during onboarding and retain the
host configuration backup. If a guarded replacement fails, inspect its reported
state before any retry.

Record successful activation evidence in the relevant PR or operational journal.
The [engineering roadmap](../engineering-roadmap.md) defines the remaining delivery
gate; repository documentation must not claim onboarding from workflow changes alone.
