# Security triage

This document describes how a security signal (a scanner result, a
dependency advisory, a researcher report, or a concern raised in review)
becomes one of: an ordinary issue, a hardening change, or a private
security advisory candidate. It is the operating procedure behind
[SECURITY.md](../../SECURITY.md); the identifier and disclosure policy it
relies on (PVK, GHSA, CVE) is defined in
[vulnerability-management.md](vulnerability-management.md), and the GitHub
settings it assumes are listed in
[github-configuration.md](github-configuration.md).

## Principle: recommend, then decide

```text
Copilot recommends; humans decide GHSA/CVE disclosure.
```

The process is deliberately conservative. It is **not**
`finding -> automatically publish GHSA/CVE`:

```text
PR / scheduled scan / researcher report
              |
              v
     deterministic tooling            (CodeQL, cargo audit, cargo deny,
              |                        Dependency Review, actionlint,
              |                        QEMU regression scenarios, ...)
              v
        security signal
              |
              v
   Copilot-assisted or manual triage  (advisory only)
              |
     +--------+-----------+----------------------+
     v                    v                      v
 ordinary issue        hardening           GHSA candidate
 (BUG)              (HARDENING)             (SECURITY / MANUAL REVIEW)
                                                 |
                                                 v
                                          human decision
                                                 |
                                                 v
                                   human opens a private advisory
                                                 |
                                                 v
                              private fix -> regression test -> security release
                                                 |
                                                 v
                          human-controlled disclosure; CVE evaluated separately
```

No automation in this repository may publish a GitHub Security Advisory,
request a CVE, publish exploit details, merge a security fix, or dismiss a
finding as non-security. An AI result is never sufficient on its own to
establish or to reject a vulnerability.

## Signals and where they surface

| Signal | Provided by | Surfaces in | Notes |
| --- | --- | --- | --- |
| CodeQL (Rust) | `security.yml` `codeql` job | Code scanning alerts | Not a required check yet; see [github-configuration.md](github-configuration.md). A static-analysis alert is a lead, not a vulnerability. |
| `cargo audit` | `security.yml` `cargo-audit` job | Job result | Fails on any unpatched RustSec advisory except the documented exception. |
| `cargo deny` | `security.yml` `cargo-deny` job | Job result | Advisories, licenses, sources, bans (`deny.toml`). |
| Dependency Review | `security.yml` (PRs only) | PR check and comment | Blocks newly introduced vulnerable dependencies. |
| actionlint | `security.yml` `actionlint` job | Check run | Workflow correctness, including reusable-workflow permissions. |
| QEMU regression scenarios | `ci.yml` | Job results | Deterministic checks of paging, CPL3 and fault behaviour. |
| Policy-file validation | `security-triage.yml` | Job result | Structure of the security-policy files only; see [below](#policy-file-validation). |
| Copilot code review | GitHub (repository setting) | PR review comments | Advisory and non-deterministic; public by default. |
| Private vulnerability reporting | GitHub | Security tab | Structured form: `.github/VULNERABILITY_REPORT.yml`. |
| GitGuardian | External GitHub App, if installed | Its own checks | No repository-side configuration exists in this repository, so its presence cannot be verified from source. A leaked secret is handled by revoking it, not by a GHSA. |

Whether a given alert is visible to the public depends on the alert type
and repository settings. Do not assume a scanner finding is private: if one
looks like a real vulnerability in a released version, avoid discussing
exploit details in public until it has been triaged.

## Classification

Every finding is classified by a human, optionally after an AI suggestion.

| Class | Meaning | Typical handling |
| --- | --- | --- |
| `BUG` | Incorrect behaviour with no security boundary or meaningful security impact. | Ordinary public issue and PR. |
| `HARDENING` | Defence-in-depth, a latent ordering hazard, or a missing mitigation, with no demonstrated boundary crossing by an attacker-controlled input. | Ordinary PR, optionally a `PVK-YYYY-NNNN` record (see the 0.0.6 audit for examples). |
| `SECURITY` | Attacker-controlled trigger, a security boundary crossed, meaningful impact, and evidence. | Private handling; GHSA evaluation. |
| `UNKNOWN` | Not enough evidence. | Investigate; do not guess. State what is missing. |

## GHSA candidate criteria

Strong GHSA consideration requires several of the following to be
supported by evidence:

```text
affected released/supported version
        +
attacker-controlled / untrusted trigger
        +
security boundary crossed
        +
meaningful security impact
        +
credible reproducer (or a rigorous architectural proof)
```

Examples of meaningful security impact:

* CPL3 to CPL0 privilege escalation;
* unauthorized kernel memory read or write;
* process isolation bypass;
* authorization or capability bypass;
* arbitrary code execution;
* remotely triggerable memory corruption;
* secret or data disclosure;
* security-relevant persistent or remotely triggerable denial of service;
* signature or secure-boot verification bypass, if such mechanisms exist.

The following are **not** a GHSA candidate merely because they exist:

* `unsafe` code;
* a panic;
* a performance defect;
* a memory leak;
* a RustSec warning (including "unmaintained");
* defense-in-depth hardening;
* a missing future mitigation.

Security relevance must be shown, not assumed. The verdict is one of:

```text
YES            the criteria above are met with evidence
NO             not a security vulnerability (record why)
MANUAL REVIEW  evidence is incomplete or the call is a judgment
```

pvKernel is early-stage; also check the "generally not treated as security
vulnerabilities" list in [SECURITY.md](../../SECURITY.md) (for example,
functionality that is documented as unimplemented). A finding fixed before
any release ships with it affected no released version and normally needs
only a PVK record and a regression test.

## CVE boundary

A CVE is evaluated separately, by a human, and only after the
vulnerability is confirmed, the affected released versions are known, the
impact is understood, a remediation and release plan exists, and the
disclosure timing has been decided. No workflow in this repository
requests a CVE. See [vulnerability-management.md](vulnerability-management.md#cve).

## Public-disclosure safety

This repository is public. Issues, pull-request comments, Copilot review
comments, Actions logs, annotations, step summaries, artifacts and commit
messages are all public.

* A suspected undisclosed vulnerability in a released version is handled
  through private vulnerability reporting, a draft repository advisory,
  and private fix branches, never a public issue.
* If a public review comment (human or Copilot) raises one, it should say
  only that security triage is required, keep exploit mechanics out, and
  point to private handling. Maintainers should move detail out of the
  thread.
* Automation must not create a public issue for a potential undisclosed
  vulnerability, and must keep its public output high-level, for example:
  *"Security triage required. Review repository security alerts / the
  private advisory workflow."*
* Copilot instructions are an **advisory control, not a confidentiality
  boundary.** Do not rely on them to keep a secret; do not paste
  unpublished exploit details into any Copilot surface that may produce
  public output.
* Even a redacted `SECURITY-TRIAGE-REQUIRED` comment reveals that a
  security concern exists at that location. Review comments normally
  attach to code in the PR under review, which is not yet released; but if
  the flagged code is already in a release, a maintainer should hide or
  delete the public comment and continue in private triage.

### Threats to the triage tooling itself

* **Instructions come from the PR head branch.** Copilot reads repository
  instructions from the branch under review, so a PR can change the
  instructions that review it. Treat any change to `.github/copilot-instructions.md`,
  `.github/instructions/`, `.github/agents/` or `.github/prompts/` as
  security-sensitive, and review it as a human.
* **Prompt injection.** Copilot processes pull-request text, code
  comments and issue text written by untrusted people. Its output can be
  steered in either direction (suppressing or inventing a finding), which
  is another reason no AI result decides anything.
* **Workflows run PR-controlled code.** `security-triage.yml` runs on
  `pull_request` with a read-only token and no secrets, so a fork PR cannot
  obtain write access or secrets through it.

## Using Copilot for triage

Supported surfaces only; none of them is a security control.

| Surface | File | Notes |
| --- | --- | --- |
| Repository instructions | `.github/copilot-instructions.md` | Used by Copilot code review and agents. Carries the disclosure rules. Copilot reads instructions from the PR head branch. |
| Path-specific instructions | `.github/instructions/*.instructions.md` | Kernel/runner Rust and workflow review checklists, scoped with `applyTo`. |
| Custom agent | `.github/agents/security-triage.agent.md` | Read-only (`read`, `search`), manual selection only. Available once the file is on the default branch, in the supported Copilot agent surfaces (agents panel on GitHub, Copilot CLI, supported IDEs). Custom agents are not documented as part of Copilot code review. |
| Prompt file | `.github/prompts/security-triage.prompt.md` | Convenience `/security-triage` command in VS Code chat. Supplementary and not relied on by anything. |

Run the agent in a **private** context (local IDE or CLI) for anything that
might be an undisclosed vulnerability. A GitHub cloud-agent session started
from a public issue can publish its output. The agent is instructed to
return only a redacted result when it cannot tell that the session is
private, but that is advisory.

Its output is the `PVK SECURITY TRIAGE` block defined in the agent file.
Treat it as a draft: verify the cited code, the affected versions and any
CWE yourself. Copilot output is non-deterministic and can be wrong in
either direction.

## Policy-file validation

`.github/workflows/security-triage.yml` runs `scripts/check_security_policy.py`
on every pull request and push to `main`, and on manual dispatch. It is
deterministic infrastructure, not a vulnerability detector, and does not
duplicate `security.yml`. It checks that:

* the required security-policy files exist;
* the Copilot instruction, agent and prompt files have valid, expected
  frontmatter, and the agent stays within its read-only tool set;
* `.github/VULNERABILITY_REPORT.yml` is a structurally valid issue form;
* no workflow contains advisory-publishing or CVE-request automation.

It uses a read-only token, no secrets, only `pull_request` (never
`pull_request_target`), and writes only a static summary. It cannot create
issues, comments, advisories or CVE requests.

The "no advisory or CVE automation" check is a **tripwire** that catches an
accidental addition by text matching; a determined author could evade a
text match. The structural control is different: the workflow
`GITHUB_TOKEN` permission set has no repository-advisory scope, so
publishing an advisory from Actions would need a separately stored PAT or
GitHub App credential. This repository must not contain one, and a change
that adds one needs its own security design and human review.

## Maintainer notifications

Use GitHub's native security notifications; do not add an SMTP service or
a third-party email action. These are per-user settings and cannot be
committed to the repository.

* Repository page: **Watch -> Custom -> Security alerts** (or All
  Activity).
* Account **Settings -> Notifications -> Subscriptions -> Watching**: make
  sure email is included for watched repositories.
* GitHub notifies repository administrators and security managers of a new
  private vulnerability report when both of the above are in place.

## Settings that live outside the repository

See [github-configuration.md](github-configuration.md) for private
vulnerability reporting, the "Require a CWE assignment" submission
requirement, Copilot automatic review, the `Protect main` ruleset and the
required status checks.
