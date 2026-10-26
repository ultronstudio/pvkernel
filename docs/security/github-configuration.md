# GitHub repository configuration

This document lists every GitHub repository/organization setting that the
workflows and policies in this repository assume, but that cannot be
configured from inside a git checkout. **None of these settings have been
applied by this document; they require a maintainer to apply them through
the GitHub UI (or `gh api`/Terraform, if preferred) and to confirm they
took effect.** Treat every setting below as "configuration prepared,
GitHub-side execution required" until a maintainer has gone through it.

## 1. Main branch ruleset

Settings -> Rules -> Rulesets -> (edit or create a ruleset targeting
`main`).

Required:

* **Require a pull request before merging** - no direct pushes to `main`.
* **Require status checks to pass** - add every job listed in
  [Required status checks](#2-required-status-checks-to-register) below.
* **Require branches to be up to date before merging** - only if the
  maintainer finds this worth the extra re-run cost; not strictly
  required with a single maintainer, but prevents merging a PR whose
  checks ran against a stale base.
* **Restrict deletions** - the ruleset must not allow deleting `main`.
* **Block force pushes** - the ruleset must not allow force-pushing
  `main`.
* **Require conversation resolution before merging** - recommended; cheap
  and catches unresolved review comments.

Explicitly **not** recommended while pvKernel has a single maintainer:
**Require review from a code owner** / requiring an approving review from
someone other than the author. A single-maintainer project cannot satisfy
"a different human must approve" without either blocking all merges or
making someone rubber-stamp their own patch, which checkbox would
undermine rather than strengthen the process. Revisit this once a second
trusted maintainer exists.

The guiding principle, restated: **GitHub should prevent a normal merge to
`main` if a required check fails.** A ruleset that only blocks deletion
and force-push (reportedly the state before this hardening pass) does not
achieve that; the required-status-checks rule above is what actually
does.

## 2. Required status checks to register

Once `.github/workflows/ci.yml` and `.github/workflows/security.yml` have
run at least once on a PR (GitHub only offers a check as a ruleset option
after it has reported at least one result), add these as **required**:

```text
Format
Clippy
Build (default)
Build (fault-tests)
Build (mem-fault-tests)
Build (supervisor-write-protection-test)
Build (qemu-test)
Build (qemu-test fault-tests)
Build (qemu-test mem-fault-tests)
Build (qemu-test supervisor-write-protection-test)
Build (userspace-syscall-test)
Build (userspace-privilege-fault-test)
Build (userspace-supervisor-fault-test)
Build (qemu-test userspace-syscall-test)
Build (qemu-test userspace-privilege-fault-test)
Build (qemu-test userspace-supervisor-fault-test)
QEMU / timer boot
QEMU / expected page fault
QEMU / post-unmap page fault
QEMU / supervisor write protection
QEMU / CPL3 syscall boundary
QEMU / CPL3 privileged-instruction fault
QEMU / CPL3 supervisor-memory fault
Dependency audit (cargo audit)
Dependency policy (cargo deny)
Workflow lint (actionlint)
Security policy files
```

The 0.0.10 correctness gate additionally reports `Host filesystem and release
tooling tests`, `QEMU / release image boot`, and all 12
`QEMU / process regression (<feature>)` matrix checks. Register these as
required after they first report. The process feature names are listed in
`.github/workflows/ci.yml`'s `qemu-process` matrix; every one must pass.

`Security policy files` is reported by `.github/workflows/security-triage.yml`,
which deliberately has no `paths:` filter so it runs on every pull request
(see [security-triage.md](security-triage.md#policy-file-validation)).
Register it only after it has reported at least once.

Note on path-filtered checks: `security.yml` runs on pull requests only when
its `paths:` filter matches. A required check from a workflow that did not
run on a given PR stays pending and blocks the merge. Before registering
the `security.yml` jobs above as required, either accept that docs-only PRs
will not satisfy them, or remove the `paths:` filter from the
`pull_request` trigger. This document does not change that behavior.

`Dependency review (pull requests only)` should also be required once it
has reported on a PR that actually changes a dependency (it is
conditioned on `github.event_name == 'pull_request'` and does not run on
every PR trigger the same way the others do, so GitHub may not always
offer it as a ruleset option depending on how recently it last ran).

**CodeQL (Rust)** is intentionally **not** in the required list above. It
is configured in `security.yml` (`build-mode: none` - the only build mode
Rust's CodeQL extractor supports; an earlier `build-mode: manual` draft
failed outright when actually run, see
[docs/audits/2026-10-pvkernel-0.0.6-pre-release.md](../audits/2026-10-pvkernel-0.0.6-pre-release.md))
and should surface findings in Code Scanning, but whether it reliably
reports a stable, consistently-named check for this specific
no_std/cross-compiled workspace across several real PRs has not been
verified GitHub-side as part of this hardening pass. Add it to the required list
once a maintainer confirms it reports consistently across several PRs;
until then, requiring it risks blocking merges on an infrastructure
hiccup rather than a real finding.

## 3. Code security configuration (Security tab)

Settings -> Code security:

* **Dependabot alerts**: enable. This is separate from
  `.github/dependabot.yml` (which only configures *update PRs*); alerts
  need to be turned on independently for Dependabot to flag vulnerable
  dependencies at all.
* **Dependabot security updates**: enable, so a vulnerable direct
  dependency gets an automatic patch PR distinct from the regular weekly
  update PRs.
* **Code scanning**: this repository drives CodeQL and the actionlint
  SARIF upload explicitly from `security.yml` ("advanced setup"). Do
  **not** also enable GitHub's "default setup" for CodeQL on this repo -
  the two can conflict (duplicate or inconsistent analysis
  configuration for the same language). If default setup was ever
  enabled previously, disable it before relying on `security.yml`'s
  CodeQL job.
* **Private vulnerability reporting**: enable. `SECURITY.md` already
  documents and links
  `https://github.com/ultronstudio/pvkernel/security/advisories/new`
  as the primary private-reporting channel; this setting is what makes
  that link actually functional instead of 404ing. Verify by visiting
  that URL as a maintainer.
* **Secret scanning** (and push protection, if available on the current
  plan): enable. Not required by this task's P0 scope, but a cheap,
  directly applicable supply-chain control for a repository that now has
  release workflows using `GH_TOKEN`.

## 4. Protected `release` Environment

Settings -> Environments -> New environment -> name it exactly `release`
(this must match the `environment: release` key used in
`.github/workflows/release.yml`'s `approval` job).

Configure:

* **Required reviewers**: add at least the current maintainer. With a
  single maintainer, this still provides a real pause-and-confirm step
  (the workflow run visibly stops and waits for an explicit "Approve" click
  in the GitHub UI, distinct from just letting the workflow run
  unattended) even though it cannot yet enforce "someone *other than* the
  author approves."
* **Prevent self-review**: enable once a second trusted maintainer
  exists. This specific option does not help with exactly one maintainer
  (there is no one else to require), but should be turned on the moment
  there is.
* **Deployment branches and tags**: optionally restrict to `main`, so the
  `release` environment (and therefore `release.yml`'s `approval`/`publish`
  jobs) can only be reached from workflow runs against `main`.

Until this environment exists and has at least one required reviewer
configured, `release.yml`'s `approval` job will **not** actually pause for
human approval - an environment with no protection rules passes through
immediately. This is exactly why `release.yml` uses `environment: release`
rather than any `if:`-based approval emulation: the workflow's behavior
correctly reflects whatever this setting is, instead of silently claiming
to enforce an approval gate that was never configured.

## 5. Immutable releases

GitHub's immutable releases feature (generally available since
2025-10-28) locks a release's tag (cannot be deleted or moved) and its
assets (cannot be added, modified, or deleted) once published.

Enable it in Settings -> General, in the repository's release-related
settings (look for an "immutable releases" toggle; GitHub has changed
exact wording/placement across preview and GA, so search Settings rather
than relying on a specific remembered path). This can be set at the
organization level instead, if preferred, to apply uniformly across
repositories.

Notes:

* Enabling it only affects **releases created after** it is enabled;
  existing releases stay mutable unless explicitly republished.
* This is GitHub's native mechanism; pvKernel does not need (and should
  not build) a custom signing/immutability scheme on top of it.
* Combined with artifact attestation (see
  [docs/security/vulnerability-management.md](vulnerability-management.md)
  and the hardening-pass final report for the exact `gh attestation
  verify` command), this gives a third party both "this tag/these assets
  cannot have silently changed" and "these exact bytes were built by this
  exact GitHub Actions run from this exact commit."

## 6. Security triage, private reporting, and Copilot review

These support [security-triage.md](security-triage.md). Source-controlled
parts (the Copilot instruction/agent/prompt files and
`.github/VULNERABILITY_REPORT.yml`) take effect once merged to the default
branch; the settings below do not live in the repository.

### Private vulnerability reporting

* **Enable** it: Settings -> Advanced Security -> Private vulnerability
  reporting. Observed state when this section was written
  (2026-10-23, read-only API query): **enabled**.
* **Require a CWE assignment**: Settings -> Advanced Security -> Private
  vulnerability reporting -> Settings -> Submission requirements ->
  "Require a CWE assignment". Recommended ON. The form file cannot enforce
  this (it offers an optional "Suggested CWE" field); the requirement is
  this setting. Its state could not be read through the API available when
  this was written, so it is **unverified**.
* **Custom form**: `.github/VULNERABILITY_REPORT.yml` (issue-form syntax).
  If GitHub cannot parse or validate it, reporters silently get the
  default form instead, so after merging, open
  `https://github.com/ultronstudio/pvkernel/security/advisories/new` as a
  non-owner or in a private window and confirm the custom fields appear.
  Once a custom form exists, reports submitted through the REST API must
  match it.

### Copilot code review

Settings that GitHub documents for this feature:

* Settings -> Copilot -> Code review -> **Use custom instructions when
  reviewing pull requests**: leave ON (the default) so
  `.github/copilot-instructions.md` and `.github/instructions/` are used.
* Settings -> Copilot -> Code review -> **Review effort level**: **Lite**
  (standard) or **Balanced** (deeper analysis of complex logic and
  security-sensitive code, at higher AI-credit cost). Balanced is the
  better fit for kernel code if the cost is acceptable.
* Automatic review is a **ruleset rule**, not a setting of its own:
  Settings -> Rules -> Rulesets -> branch ruleset targeting the default
  branch -> "Automatically request Copilot code review", with "Review new
  pushes" ON (and optionally "Review draft pull requests").

Prepared change, not applied: create a **separate** branch ruleset (for
example named `Copilot review`, targeting the default branch, enforcement
Active) containing only that rule with "Review new pushes" enabled. A
separate ruleset avoids editing `Protect main`. Copilot review is advisory
and non-deterministic; it never replaces human review and must not be added
as a required check.

Availability of these controls depends on the account's Copilot plan.

### Maintainer notifications

Per-user settings; they cannot be committed. Watch the repository with
**Custom -> Security alerts** (or All Activity) and make sure email is
included under Settings -> Notifications -> Subscriptions -> Watching.
GitHub uses this to notify administrators and security managers of new
private vulnerability reports. No SMTP service or email action is used by
any workflow.

### Observed ruleset state

Read-only API query on 2026-10-23: the repository has one ruleset,
`Protect main` (target: default branch, enforcement: active; repository
admin role may bypass). Its rules were `deletion` and `non_fast_forward`
only. It had **no** "Require a pull request before merging" rule and **no**
"Require status checks to pass" rule, which are the controls section 1
above calls for. This pass did not modify the ruleset.

## Verification status

Every setting above is a **recommendation**, not a confirmed/enabled
state, except where a section above says "Observed state". The only
repository settings read while preparing the security-triage change were
read-only API queries (private vulnerability reporting status and the
ruleset's rule types). No repository setting was modified; the
"Observed" notes are a point-in-time snapshot and should be re-checked in
the GitHub UI.
