# pvKernel 0.0.6 pre-release hardening audit report

**Date:** 2026-10-23
**Scope:** P0 correctness/release-engineering hardening and final 0.0.6
release-blocking paging security fixes (BootInfo/MemoryRegions ownership,
paging prerequisite validation, NXE/CR0.WP enforcement, toolchain pinning,
CI/security/release infrastructure). No 0.0.7 kernel feature work (heap,
scheduler, processes, ELF loader, VFS, APIC/SMP, new drivers) was in scope
or performed.
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
| Four GitHub Action references (`cargo-deny-action`, `action-actionlint`, `codeql-action`, `attest-build-provenance`) were pinned to the SHA of their annotated **tag object**, not the commit the tag points to | Found during final review, cross-checked against GitHub's API (`git/refs/tags/<tag>` returning `"type":"tag"` for each) | Yes - `gh api repos/<owner>/<repo>/commits/<sha>` returned HTTP 422 "No commit found" for all four pinned SHAs before the fix | `gh api repos/EmbarkStudios/cargo-deny-action/commits/b66acf5e9fe20f8aba065be86778a8a4c846f902` (and the other three), pre-fix | Correctness (would have failed to resolve at workflow run time, not a vulnerability) | Yes - repinned all four to the commit each tag object peels to, re-verified each resolves and matches | `gh api .../commits/<new-sha>` succeeds and matches for all four; confirmed by the actual GitHub Actions runs on the pushed commit |
| After push: `ci.yml`'s Clippy job and `security.yml`'s `actionlint`/`codeql` jobs actually failed on GitHub Actions, despite passing locally | Actual GitHub Actions run on the pushed commit (`gh run view --log-failed`), not merely a local check | Yes - read the actual failure logs for all three jobs | See the three rows immediately below | Correctness (would have broken the CI/security gates on every future push, not a vulnerability) | Yes - see the three rows immediately below | Re-ran `gh run list`/`gh run view` after the follow-up push; all three jobs green |
| `cargo clippy --workspace --locked -- -D warnings` failed on GitHub's runner only, with `error[E0152]: found duplicate lang item 'panic_impl'` (`std` pulled in via `bitflags`) - selecting `kernel` and `pvkernel-runner` as two simultaneous `--workspace` root units let Cargo's feature resolver unify a `std`-enabling feature across their dependency graphs on at least one platform/toolchain combination, even though a local clean build on Windows did not reproduce it | GitHub Actions run on the pushed commit | Yes - read the actual CI failure log | `cargo clippy --workspace --locked -- -D warnings` on GitHub Actions (ubuntu-latest) | Correctness (would have permanently broken the Clippy required check) | Yes - replaced with two single-package invocations (`cargo clippy -p kernel --locked --target x86_64-unknown-none -- -D warnings` and `cargo clippy -p pvkernel-runner --locked -- -D warnings`), which never select two root units together | Re-ran both corrected commands locally (clean build) and via a follow-up push; both pass |
| `build-mode: manual` is not a supported CodeQL build mode for Rust: the `codeql` job failed with "A fatal error occurred: ... Rust does not support the manual build mode. Please try using one of the following build modes instead: none." | GitHub Actions run on the pushed commit | Yes - read the actual CodeQL `init` step failure log | `github/codeql-action/init` with `languages: rust, build-mode: manual` on GitHub Actions | Correctness (contradicted this pass's own `docs/security/github-configuration.md` claim that `manual` was the deliberate, safer choice) | Yes - changed to `build-mode: none` (the only mode Rust's extractor supports) and removed the now-unnecessary explicit `cargo build --locked` trace step | Re-ran via a follow-up push; `codeql` job green |
| `reviewdog/action-actionlint@v1` has no `sarif` input (its actual inputs are `entryPoint`, `args`, `github_token`, `tool_name`, `level`, `reporter`, `filter_mode`, `fail_level`, `fail_on_error`, `reviewdog_flags`, `actionlint_flags`); the `actionlint` job's SARIF-upload step then failed with "Path does not exist: actionlint-results.sarif" | GitHub Actions run on the pushed commit, and `gh api repos/reviewdog/action-actionlint/contents/action.yml` | Yes - read the actual action.yml input list and the failure log | `reviewdog/action-actionlint@v1` with `sarif: actionlint-results.sarif` on GitHub Actions | Correctness (the SARIF-to-Code-Scanning path never worked) | Yes - switched to `reporter: github-check` (works for every trigger this workflow has, unlike the PR-only reporters) and removed the SARIF upload step and its now-unneeded `security-events: write` permission (replaced with `checks: write`) | Re-ran via a follow-up push; `actionlint` job green |
| actionlint itself (once its own job could run) statically flagged: `release.yml`/`release-candidate.yml`'s `security:` job grants `pull-requests: none` to the reusable `security.yml`, but `security.yml`'s `dependency-review` job needs `pull-requests: write` - checked regardless of that job's runtime `if: github.event_name == 'pull_request'` condition | `actionlint`'s workflow-call permission check, GitHub Actions run on the pushed commit | Yes - read the actual actionlint annotation | `actionlint .github/workflows/release.yml .github/workflows/release-candidate.yml` | Correctness (static permission-mismatch, not a vulnerability) | Yes - added `pull-requests: write` and `checks: write` to both `security:` caller jobs' permission blocks | Re-ran `actionlint` locally and via a follow-up push; no more workflow-call permission findings |
| NX/NXE was detected and logged but not required, allowing boot to continue without the execute-disable mechanism required by the paging/W^X model | Existing 0.0.6 pre-release audit, former follow-up finding 1 | Yes - reviewed `memory::validate_paging_prerequisites`, `nx_enabled`, and early `memory::init` ordering | Boot with `IA32_EFER.NXE` absent would fail the explicit prerequisite assertion | Release-blocking paging invariant | Yes - `validate_paging_prerequisites()` requires `IA32_EFER.NXE` before address-space initialization; the bootloader remains responsible for enabling it | Existing boot QEMU scenarios exercise the supported bootloader configuration; missing NXE fails explicitly before address-space setup. Local QEMU execution is unavailable in this environment; see verification below. |
| CR0.WP was not established or checked, so supervisor writes could bypass read-only page-table permissions | Existing 0.0.6 pre-release audit, former follow-up finding 2 | Yes - reviewed `memory::validate_paging_prerequisites` and the CR0 setup path | `qemu-test supervisor-write-protection-test` writes from CPL0 to a present supervisor read-only page and requires the exact protection-violation error semantics and CR2 address | Release-blocking paging invariant | Yes - early paging validation enables CR0.WP when necessary and verifies it before address-space initialization | Dedicated `QEMU / supervisor write protection` CI scenario, plus all existing paging and CPL3 fault scenarios |

## Rejected

None. Every hypothesis investigated this session (the P0 items and the
two clippy/audit CLI issues found while implementing them) was verified
as a real, reproducible problem before being recorded above; nothing was
raised and then dismissed as a false alarm.

## Follow-up findings (P1 / P2 / informational)

These are explicitly **not** fixed in this pass. Recorded here so pvKernel
0.0.7 planning starts from a known list instead of rediscovering them.

| # | Finding | Category | Evidence |
|---|---|---|---|
| 1 | `address_space::map_new_page` allocates a physical frame, then calls `paging::map_page`; if mapping fails, the already-allocated frame is dropped via `?` and never returned to the allocator. This matches the current monotonic allocator model: no `deallocate_frame` exists, and `unmap_page` also cannot return frames for reuse. It becomes load-bearing when real frame reclamation is introduced. | Deferred to 0.0.7 | `address_space.rs`; `BootFrameAllocator` has no reclamation API. Do not add partial frame reclamation to 0.0.6. |
| 2 | `BootFrameAllocator`'s frame iteration (`nth(self.next)`) re-walks from the start of the region iterator on every allocation, O(n) per allocation and O(n²) cumulative. Allocation volume remains low before heap work; this is performance cleanup, not a current correctness defect. | Deferred to 0.0.7 | `BootFrameAllocator` implementation; no measured 0.0.6 regression. |
| 3 | Existing CPL3 fault scenarios do not all assert every possible detail such as exact fault RIP; the CPL3 supervisor-page scenario does verify its expected address and error-code bits, and the new CPL0 test asserts its origin and full required protection-fault classification. | P2 / follow-up | Review of `interrupts.rs` expected-fault matchers; no release-blocking defect identified. |
| 4 | CodeQL's Rust analysis check name should be observed across additional real PRs before adding it to the required-status-check list. | Informational | `docs/security/github-configuration.md` intentionally excludes it pending repeated GitHub-side confirmation. |
| 5 | Several transitive dependencies have duplicate major/minor versions (`bitflags`, `getrandom`, `syn`) and are reported by `cargo deny check` as warnings under the configured policy. | Informational | `deny.toml` permits multiple versions as warnings. |

## Verification method

### Paging-hardening update verification

The following checks were run for this update:

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy -p kernel --locked --target x86_64-unknown-none -- -D warnings` | Passed |
| `cargo clippy -p pvkernel-runner --locked -- -D warnings` | Passed |
| `cargo test --locked` | Passed (the runner reports 0 unit tests) |
| `cargo check --locked --features "qemu-test supervisor-write-protection-test"` | Passed |
| `cargo check --locked --features supervisor-write-protection-test` | Passed |
| Builds for the new scenario and existing expected-page-fault, post-unmap, CPL3 privilege, CPL3 supervisor-memory, and syscall QEMU feature sets | Passed |
| QEMU execution of the memory/fault scenarios | Not run locally: `qemu-system-x86_64` is not installed; CI now includes `QEMU / supervisor write protection` alongside the existing scenarios |
| `git diff --check` | Passed |

No dependency manifests or lockfiles were changed. The QEMU behavior remains
to be confirmed by the repository's GitHub Actions run after push.

Every "Verified" claim above reflects an actual command run against this
repository during this session (see the final report's verification
table for the full list and exit status of each), not a description
taken at face value. Where GitHub-side behavior (required-check
reporting, CodeQL analysis, environment approval gating, artifact
attestation) could not be exercised from a local checkout, this report
and the final hardening report both say so explicitly rather than
claiming it was tested.

The first push of this hardening pass *was* exercised on GitHub Actions
(not just locally), and that is exactly what caught the Clippy/CodeQL/
actionlint failures recorded above: all three passed every local check
on this session's development machine, and only failed once actually run
on `ubuntu-latest`. This is recorded as a lesson for future sessions on
this repository: a workflow that only "looks correct" or that has only
been run locally is not verified; treat anything that touches GitHub
Actions' own runners, permission model, or third-party action behavior as
unconfirmed until it has actually run there.
