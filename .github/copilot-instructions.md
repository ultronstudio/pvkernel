# pvKernel – Copilot instructions

pvKernel is an experimental, security-sensitive `no_std` x86_64 kernel in
Rust. Review and write code against the invariants in
[ARCHITECTURE.md](../ARCHITECTURE.md) and [docs/architecture/](../docs/architecture/).
Do not assume a mechanism exists because a mature kernel would have it:
check the code and docs first, and never invent an invariant, ABI or
feature.

## Disclosure safety (highest priority)

Pull-request review comments are public. When a finding appears to be a
security vulnerability that affects a released or supported version
(see [SECURITY.md](../SECURITY.md)):

- Mark the comment `SECURITY-TRIAGE-REQUIRED`.
- Say only: *potential release-impacting security issue; move detailed
  analysis to private security triage / a repository security advisory.*
  Naming the broad boundary or category is fine.
- Do NOT include a proof of concept, exploit recipe, attack chain, exact
  trigger values, register/address layouts, or secrets.
- Do not claim exploitability, a CVE or a GHSA. Do not suggest opening a
  public issue.

These instructions are an advisory control, not a confidentiality
boundary. Ordinary bugs and hardening ideas may be commented on normally.
Triage procedure: [docs/security/security-triage.md](../docs/security/security-triage.md).

## Security boundaries to consider

CPL3 to CPL0, userspace to kernel, process to process, device/DMA to
kernel memory, filesystem and user permissions, and any future
capability or handle boundary.

## Paging invariants the code enforces today

- `CR4.LA57 == 0` and `IA32_EFER.NXE == 1`: boot panics otherwise.
  `CR0.WP == 1`: set if clear, then verified, and boot panics if it still
  is not set (all in `memory::validate_paging_prerequisites`).
- Writable implies non-executable; executable implies not writable
  (`paging::MemoryError::InvalidPermissions`).
- Kernel mappings are never `USER_ACCESSIBLE`; user mappings are
  `USER_ACCESSIBLE` only where explicitly requested.

Flag any change that weakens a check, reorders it after code that relies
on it, or turns a failed prerequisite check into a warning.

## Always check

- Every `unsafe` block needs a truthful `// SAFETY:` comment (see
  [CONTRIBUTING.md](../CONTRIBUTING.md) §7).
- Test-only scaffolding (`qemu-test`, `*-test` features, expected-fault
  matchers) must not change default-build behaviour.
- Scope: reject unrelated changes, dependency bumps and weakened CI or
  security gates in the same PR.

Path-specific checklists live in `.github/instructions/`.
