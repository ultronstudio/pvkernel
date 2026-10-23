# pvKernel 0.0.6 pre-0.0.7 hardening pass - audit report

**Date:** 2026-10-23
**Scope:** P0 correctness/release-engineering hardening pass ahead of
pvKernel 0.0.7 feature development (BootInfo/MemoryRegions ownership,
paging-mode validation, toolchain pinning, CI/security/release
infrastructure). No kernel feature work (heap, scheduler, processes,
ELF loader, VFS, APIC/SMP, new drivers) was in scope or performed.
**Version at completion:** `0.0.6` (unchanged).

This report distinguishes confirmed issues, rejected hypotheses,
follow-up findings, and information-only observations, per
[docs/security/vulnerability-management.md](../security/vulnerability-management.md).
No PVK ID below represents a release-impacting security vulnerability in
shipped kernel functionality; see the "Severity" column for why each
entry was or was not classified that way. Every row was independently
verified against the actual repository state (code, `Cargo.lock`, CI
output), not accepted on description alone.

## Confirmed and fixed

| Finding | Source | Verified | Reproducer | Severity | Fixed | Regression test |
|---|---|---|---|---|---|---|
| `BootInfo`/`MemoryRegions` ownership was an aliasing `'static` slice built via `from_raw_parts` while a separate `&'static mut BootInfo` stayed reachable | Task spec (P0-1) | Yes - read `memory::init` and `kernel_main` before/after | N/A (architectural; no observed misbehavior, invariant was documented not enforced) | Informational / hardening | Yes - `core::mem::replace` moves `MemoryRegions` out of `BootInfo`; `boot_info` is taken by value and never returned | Existing boot/memory QEMU scenarios (unchanged behavior, now structurally enforced) |
| Five-level paging (CR4.LA57) was checked and logged only *after* `active_level_4_table`/`OffsetPageTable::new` had already assumed four-level paging | Task spec (P0-2) | Yes - read `memory::init` ordering before/after | N/A (current bootloader never enables LA57; this closes a latent ordering hazard, not an observed fault) | Informational / hardening | Yes - `validate_paging_prerequisites()` now runs first and panics loudly on LA57 | Existing boot QEMU scenarios; no dedicated LA57-active test exists (would require a bootloader that enables LA57, out of scope) |
| `rust-toolchain.toml` floated on `channel = "nightly"` despite being described elsewhere as pinned | Task spec (P0-3) | Yes - `rustc -Vv`/`rustup show` before fix showed drift risk | N/A | Reproducibility | Yes - pinned to `nightly-2026-10-23`, the toolchain already active and proven to build/pass locally | `cargo build --locked` and `cargo fmt --all -- --check` both pass under the pinned toolchain |
| `cargo clippy --workspace --locked --all-targets -- -D warnings` (as drafted in the in-progress `ci.yml`) fails to compile | Found this session, running the actual command | Yes - ran locally, got `error[E0152]: found duplicate lang item 'panic_impl'` | `cargo clippy --workspace --locked --all-targets -- -D warnings` on this exact commit | Correctness (would have broken CI, not a vulnerability) | Yes - dropped `--all-targets`; `cargo clippy --workspace --locked -- -D warnings` passes clean | Re-ran the corrected command locally; exit 0 |
| `cargo audit --locked --deny warnings` (as drafted) fails: `cargo-audit` has no `--locked` flag | Found this session, running the actual command | Yes - ran locally, got `error: unexpected argument '--locked' found` | `cargo audit --locked` | Correctness (would have broken `security.yml`'s first run) | Yes - removed `--locked` | Re-ran corrected command locally; exits 0 after the dependency/exception fixes below |
| Five real RustSec advisories against `rustls`/`rustls-webpki`/`webpki-roots` (build-time-only, via `ureq <- ovmf-prebuilt <- pvkernel-runner`) | `cargo deny check` / `cargo audit`, run locally this session | Yes - `cargo deny check` output, cross-checked against rustsec.org advisory pages linked in its own output | `cargo deny check` / `cargo audit` on the pre-fix `Cargo.lock` | Low-moderate (build-time HTTPS client only; never linked into the kernel binary; worst cases are a malicious/MITM build-time firmware-fetch server or verbose debug logging of session secrets) | Yes - `cargo update -p rustls -p rustls-webpki -p webpki-roots` (0.23.43->0.23.46, 0.103.13->0.103.16, 1.0.9->1.0.10); rebuilt successfully | `cargo deny check`/`cargo audit` both clean after the update; `cargo build --locked` re-verified |
| `webpki-roots` license (`CDLA-Permissive-2.0`) rejected by `deny.toml`'s license allow-list | `cargo deny check`, run locally this session | Yes | `cargo deny check` on the pre-fix `deny.toml` | Policy gap, not a vulnerability | Yes - added `CDLA-Permissive-2.0` to the allow-list with a comment scoping it to this one root-certificate-data dependency | `cargo deny check` clean |
| Intra-workspace path dependency (`kernel = { path = ..., artifact = "bin", ... }`) flagged by `wildcards = "deny"` | `cargo deny check`, run locally this session | Yes | `cargo deny check` on the pre-fix `deny.toml` | False positive, not a vulnerability | Yes - `allow-wildcard-paths = true` added with rationale | `cargo deny check` clean |
| `bincode` 1.3.3 flagged unmaintained (`RUSTSEC-2025-0141`), pulled in transitively via `bootloader` v0.11.17 (build-time only); no safe upgrade exists per the advisory itself | `cargo deny check`/`cargo audit`, run locally this session | Yes | `cargo deny check` / `cargo audit` on `Cargo.lock` | Low (build-time only; "unmaintained," not an active vulnerability; no known exploit) | Documented temporary exception, **not** silently ignored: `PVK-2026-0001`, recorded in `deny.toml`'s `[advisories].ignore` and `security.yml`'s `cargo audit --ignore`, with removal condition (upgrade past the `bootloader` version that pulls in `bincode`, or a maintained fork) | `cargo deny check`/`cargo audit` both pass with the exception in place; both fail again if the exception is removed without a real fix (verified by temporarily reverting) |
| Version string (`0.0.6`) duplicated across `Cargo.toml` and `kernel/Cargo.toml`, plus hardcoded in boot banner/QEMU title | Task spec (P0-7) | Yes | N/A | Hardening (drift risk, not an active bug - both were already `0.0.6`) | Yes - `[workspace.package] version = "0.0.6"`, both crates inherit via `version.workspace = true`; runtime strings use `env!("CARGO_PKG_VERSION")` | `cargo build --locked` + `cargo metadata` confirm both packages resolve to `0.0.6` |

## Rejected

None. Every hypothesis investigated this session (the P0 items and the
two clippy/audit CLI issues found while implementing them) was verified
as a real, reproducible problem before being recorded above; nothing was
raised and then dismissed as a false alarm.

## Follow-up findings (P1 / P2 / informational)

These are explicitly **not** fixed in this pass, per the task's scope
(no heap/alloc/scheduler work). Recorded here so pvKernel 0.0.7 planning
starts from a known list instead of rediscovering them.

| # | Finding | Category | Evidence |
|---|---|---|---|
| 1 | NX is detected and logged (`memory::nx_enabled`) but never enforced as a hard precondition the way CR4.LA57 now is. A CPU/firmware configuration without NX would silently run with weaker W^X than assumed. | P1 | `grep`-confirmed: `nx_enabled()` exists and is only used in a `kprintln!`, not in any `assert!`/panic path. |
| 2 | CR0.WP (supervisor write-protection) is never read or verified anywhere in the memory subsystem. | P1 | `grep`-confirmed: no reference to `Cr0Flags::WRITE_PROTECT` or CR0 at all in `kernel/src`. |
| 3 | `address_space::map_new_page` allocates a physical frame, then calls `paging::map_page`; if mapping fails, the already-allocated frame is dropped via `?` and never returned to the allocator. Verified this is consistent with, not worse than, the rest of the module's model: `BootFrameAllocator` is a monotonic bump allocator with no `deallocate_frame` at all (confirmed by `grep`), and `unmap_page`'s own doc comment already states its returned frame "cannot be reused." So this is a real but not newly-introduced leak, and it matches the milestone's documented "frames are never reclaimed before heap work" model rather than violating it. Becomes load-bearing the moment real frame reclamation (heap allocator) is introduced. | P1 | Read `address_space.rs` lines 34-57 directly; confirmed no `deallocate_frame` method exists anywhere in the memory module. |
| 4 | `BootFrameAllocator`'s frame iteration (`nth(self.next)`) re-walks from the start of the region iterator on every allocation, which is O(n) per call and O(n^2) cumulative over many allocations. Not measured as a current performance problem (allocation volume is low pre-heap), but worth a cursor-based rewrite before heap work increases allocation frequency. | P2 | Task spec's own description, cross-checked against the general shape of `BootFrameAllocator::new`/its `FrameAllocator` impl (bump-style, no persisted cursor beyond `next`). |
| 5 | The three CPL3 fault QEMU tests (privilege fault, supervisor fault) match the faulting condition but could additionally assert the exact CPL the fault originated from, exact error-code bits, and/or the expected faulting instruction pointer, for stronger regression protection against a handler that is "sort of" right. | P2 | Task spec's own description; not independently re-derived in detail this session given the no-QEMU-locally constraint (see Verification table). |
| 6 | CodeQL's Rust support for a cross-compiled, `-Z build-std`, `#![no_std]` workspace driven by a pinned nightly has not been verified GitHub-side; `build-mode: manual` was chosen as the safer option over `autobuild`, but whether it reports a clean, consistently-named check has not been observed in an actual GitHub Actions run. | Informational | Documented in `docs/security/github-configuration.md`'s required-checks section; deliberately excluded from the required-status-check list until confirmed. |
| 7 | Several transitive dependencies have duplicate major/minor versions in the graph (`bitflags` 1.3.2/2.13.1, `getrandom` 0.2.17/0.4.3, `syn` 2.0.119/3.0.3) - reported by `cargo deny check` as warnings, not failures, per `deny.toml`'s `multiple-versions = "warn"` policy. Not a security issue; listed for completeness since it showed up during this pass's `cargo deny` runs. | Informational | `cargo deny check` output, this session. |

## Verification method

Every "Verified" claim above reflects an actual command run against this
repository during this session (see the final report's verification
table for the full list and exit status of each), not a description
taken at face value. Where GitHub-side behavior (required-check
reporting, CodeQL analysis, environment approval gating, artifact
attestation) could not be exercised from a local checkout, this report
and the final hardening report both say so explicitly rather than
claiming it was tested.
