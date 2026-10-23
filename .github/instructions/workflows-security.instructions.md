---
applyTo: ".github/workflows/**,.github/actions/**"
---

# GitHub Actions security review checklist

Complements `.github/copilot-instructions.md`. This repository is public;
anything a workflow prints, annotates, uploads or comments is public.

- **Least privilege:** a top-level `permissions: contents: read` default;
  elevate per job only for a named reason. Flag `write-all`, new
  `contents: write`, `pull-requests: write`, `issues: write` or
  `id-token: write` that the job does not demonstrably need.
- **Pinning:** third-party actions are pinned to a full commit SHA with a
  trailing version comment, matching existing workflows. Flag tags or
  branches, and verify a pinned SHA is a commit rather than an annotated
  tag object.
- **Untrusted input:** PR titles, branch names, commit messages, issue
  text and `github.event.*` fields must never be interpolated into `run:`
  scripts. Pass them through `env:` and quote the shell variable.
- **Triggers:** flag `pull_request_target`, `workflow_run` or
  `issue_comment` triggers that check out or execute PR-head code, or that
  run with secrets or a write token on fork PRs.
- **Secrets:** no secret in logs, artifacts, cache keys, step summaries or
  `echo`; no new PAT or GitHub App credential.
- **Disclosure:** no workflow may create a public issue, comment or
  annotation containing details of a suspected undisclosed vulnerability,
  publish or create a repository security advisory, or request a CVE.
  Public output about a security signal must stay high-level.
- **Provenance:** release artifacts are built from the checked-out commit
  and attested; do not weaken or skip attestation, SBOM or checksum steps.
- **Gates:** do not remove, relax, `continue-on-error` or add `|| true` to
  a security or CI job (`cargo audit`, `cargo deny`, Dependency Review,
  actionlint, CodeQL, QEMU scenarios) to make a run green. A new
  `--ignore` or `deny.toml` exception needs a documented PVK entry.
- **Reusable workflows:** a caller's `permissions:` must cover every
  permission the called workflow's jobs request.
