---
description: Run a pvKernel security triage on a finding (CodeQL alert, RustSec/cargo-audit, dependency advisory, audit finding, private report, or PR review concern).
agent: security-triage
argument-hint: describe or paste the finding (do not paste secrets or exploit code)
---

Triage this finding for pvKernel:

${input:finding:Describe the finding, its source, and the files or versions involved}

Follow the `security-triage` agent procedure and output format, and use the
repository's own policy rather than general assumptions:

- `SECURITY.md` for supported versions and scope;
- `docs/security/security-triage.md` for classification and GHSA criteria;
- `docs/security/vulnerability-management.md` for PVK / GHSA / CVE policy;
- `ARCHITECTURE.md` and `docs/architecture/` for the invariants involved.

Reminders:

- This is a recommendation only. Do not create or edit an advisory, do not
  request a CVE, and do not write files, issues or comments.
- If the finding may be an undisclosed vulnerability in a released
  version, keep the output free of proof-of-concept detail, and say so if
  this session may be public.
- Finish with the required `PVK SECURITY TRIAGE` block.

This prompt file is a convenience for VS Code chat. No security control in
this repository depends on it.
