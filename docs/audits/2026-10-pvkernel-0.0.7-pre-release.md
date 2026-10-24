# pvKernel 0.0.7 Pre-release Audit

**Audit date:** 2026-10-24

**Audited `origin/main`:** `53dedd38b43e7aa12a4175390a9af2dc9ccbb3a1`

**0.0.7 feature head:** `26e002a6218a95d27110ed5d18dce4b2d80b7565`

**Baseline:** `v0.0.6`, `036e5cb3d235a881dcf2f7ff117c425e86b3d593`

## Scope

This audit reviews the complete `v0.0.6..origin/main` delta for pvKernel
0.0.7: stateful physical-frame allocation, the fixed kernel heap, the bitmap
global allocator, Rust `alloc` integration, QEMU coverage, documentation, and
release/security workflow integration. It does not implement or audit 0.0.8
scheduler work.

The audited source is exactly the merged PR #23 state at
`53dedd38b43e7aa12a4175390a9af2dc9ccbb3a1`. The audit checkout was created
from that SHA. A local-only `main` commit (`5ea79d6`, `.gitignore` housekeeping)
was left intact on its existing branch and is not part of the audited delta.

The 17 files in the 0.0.6-to-0.0.7 delta are:

```text
.github/workflows/ci.yml
.gitignore
ARCHITECTURE.md
CHANGELOG.md
Cargo.lock
Cargo.toml
README.md
ROADMAP.md
docs/architecture/virtual-memory.md
docs/development/memory-testing.md
kernel/Cargo.toml
kernel/src/arch/x86_64/interrupts.rs
kernel/src/arch/x86_64/memory/allocator.rs
kernel/src/arch/x86_64/memory/frame_allocator.rs
kernel/src/arch/x86_64/memory/heap.rs
kernel/src/arch/x86_64/memory/mod.rs
kernel/src/main.rs
```

## Audited commit

`origin/main` was `53dedd38b43e7aa12a4175390a9af2dc9ccbb3a1`, the merge commit
for PR #23. It contains feature head
`26e002a6218a95d27110ed5d18dce4b2d80b7565`. No later `origin/main` commit was
present when the audit began.

## Executive summary

No P0 or P1 kernel correctness or security blocker was found. The heap range,
page permissions, bitmap access bounds, valid `GlobalAlloc` allocation and
deallocation paths, interrupt masking, OOM reporting, and physical ownership
model are consistent with the documented single-CPU design. The old repeated
frame-map rescans are gone. All ten deterministic QEMU scenarios and the
requested local validation commands passed, apart from the known Windows
hostile-filename fixture limitation in the Python unit suite.

At the time of this audit, three P2 release-process/defensive-hardening
observations remained. The RC dispatch observation was subsequently resolved
before RC1 by commit `1a92cd557a6b7106022284d48b3641a62e862a91`; two P2
follow-ups remain. The recommendation at the audited commit was
**CONDITIONALLY READY** for `v0.0.7-rc.1`, subject to the dispatch checks
described below. This is not stable-release approval.

## Architecture changes since 0.0.6

The frame allocator advances a persistent cursor through bootloader-supplied
usable regions instead of restarting an iterator and applying `.nth(n)` on
every allocation. Heap setup reserves 1 MiB at a fixed canonical virtual
address, maps 256 4 KiB pages, and keeps an unmapped guard page on each side.
Heap pages use the existing kernel-data permission abstraction.

Rust `alloc` uses a first-fit bitmap allocator. The heap is divided into 16-byte
units; static occupancy and allocation-start bitmaps are stored with the
global allocator, outside heap allocations. The allocator supports reuse and
alignment-aware allocation. Adjacent free units automatically form larger
available runs; there are no intrusive free-list nodes in the final design.

The kernel remains `no_std`, single-CPU, fixed-heap, and monotonic with respect
to physical frames. This milestone does not add general frame reclamation,
heap growth, per-process heaps, or a slab allocator.

## Threat model

The bootloader-provided memory map and boot-time page tables are trusted inputs
under the `bootloader`/`bootloader_api` contract. Only regions classified
`Usable` are considered for physical frames. The map is moved out of
`BootInfo`, then retained as the allocator's shared, immutable source. The
allocator assumes no second physical allocator hands out frames from the same
map.

The heap is supervisor-owned kernel memory. No user process or untrusted
userspace allocator exists in this milestone. The review focuses on
accidental kernel memory overlap, invalid allocator arithmetic, overlapping
live allocations under valid `GlobalAlloc` use, incorrect page permissions,
interrupt reentrancy, and deterministic failure behavior.

The [pinned bootloader API](https://docs.rs/bootloader_api/0.11.17/bootloader_api/info/struct.BootInfo.html)
documents that regions marked `Usable` can be freely used by the kernel and
that bootloader-used regions are marked separately. The
[bootloader changelog](https://docs.rs/crate/bootloader/0.11.17/source/Changelog.md)
records sorting of UEFI memory maps in 0.11.4. The current boot path is UEFI.
The allocator follows the supplied order and does not normalize or sort it;
correctness therefore continues to depend on the bootloader supplying a
valid, disjoint memory map.

## Heap virtual-memory review

The configured addresses are:

```text
lower guard: 0xffff90000ffff000
HEAP_START:  0xffff900010000000
HEAP_SIZE:   0x00100000 (1 MiB)
HEAP_END:    0xffff900010100000
upper guard: 0xffff900010100000
```

The start and size are page-aligned. `HEAP_END` is computed with checked
constant arithmetic, and runtime setup checks its checked recomputation,
canonical addresses, page offsets, lower-guard subtraction, and end-page
arithmetic. On the supported four-level paging configuration, the range and
both guard addresses are canonical.

Before mapping, setup translates both guards and every heap page and requires
`NotMapped`. The active translator distinguishes a huge-page translation or
unsupported frame from an ordinary unmapped address, so a huge mapping
covering any portion also fails the check. Every page is then mapped through
`map_new_page`; any allocation or mapping error panics/halts before allocator
initialization. A consumed physical frame on a fatal map failure is not
rolled back, matching the monotonic frame ownership model.

Because all 256 pages are mapped by the same loop with the same permission
constant, checking first, middle, and last translations is reasonable runtime
verification of the mapping path. Both guards are checked again after
mapping. Only the lower guard is deliberately faulted in the destructive
QEMU scenario; the upper guard is verified unmapped without a destructive
access. This is sufficient for this fixed, uniformly mapped range.

## Bitmap allocator review

The final allocator is in
[`allocator.rs`](../../kernel/src/arch/x86_64/memory/allocator.rs). It uses
first-fit search over fixed 16-byte units. `occupied` records in-use units;
`allocation_starts` marks each allocation's first unit. No heap-resident
metadata or raw pointer dereference is used by allocator bookkeeping.

`allocate` finds a free run large enough for the rounded size at an address
aligned to `Layout::align()`, then marks that complete run while holding the
mutex. It never marks the alignment gap. A collision advances the candidate
past the occupied unit; the candidate is monotonic and bounded by
`self.units`, so the search terminates. An allocation that cannot fit after
alignment returns null without changing bitmap state.

`deallocate` validates initialization, non-null pointer, checked subtraction
from heap start, 16-byte unit alignment, the supplied layout alignment, unit
bounds, occupancy, and the allocation-start bit. It scans only within the
heap, stopping at a free unit or the next allocation-start bit. It compares
the discovered run length to the layout's rounded unit count before clearing
any bits. Therefore a correct deallocation of allocation A stops before
immediately adjacent allocation B, and the clear range cannot include B.
Interior pointers, double frees, outside pointers, and size mismatches that
change the rounded unit count do not mutate state.

First-fit complexity is linear in the units inspected and requested run. For
this fixed 65,536-unit heap, this is simple and bounded; no claim of constant
time is made. The bitmap gives natural splitting/reuse/coalescing semantics:
allocated units separate free runs, and clearing adjacent runs makes a larger
run available without list-node surgery.

## GlobalAlloc / alloc integration review

The valid [`GlobalAlloc` contract](https://doc.rust-lang.org/core/alloc/trait.GlobalAlloc.html)
is supported: successful allocations are
within the configured heap and meet the requested alignment; failure returns
null; deallocation with the matching pointer and `Layout` frees the run. The
unsafe `dealloc` caller must supply a pointer and layout from this allocator.
Invalid unsafe callers are handled defensively when detectable, but that is
not part of the safe caller guarantee.

One defensive limitation is that allocator metadata stores unit occupancy and
allocation starts, not each original byte size or alignment. Two different
sizes that round to the same 16-byte unit count (for example 17 and 18 bytes)
are indistinguishable to `deallocate`; if an unsafe caller supplies that
wrong-size layout, it can free the run. Correct Rust `alloc` callers pass the
matching layout, so this does not break valid `Box`, `Vec`, or `String` use.
This is recorded as P2 hardening rather than a valid-usage memory-safety
finding. If exact invalid-layout rejection is desired, retain the original
layout metadata outside heap objects.

`extern crate alloc` and `#[global_allocator]` are enabled. `heap::init` runs
after paging prerequisites, frame allocator, and active page-table setup; the
global allocator is initialized only after all mappings and permissions have
been checked. `run_heap_smoke_tests` runs after that initialization. No
heap-backed value is created before allocator initialization in the boot
path. The heap tests create and drop `Box`, `Vec`, and `String` values and
exercise reuse and fragmentation.

## Heap permissions review

Heap permissions come from `Permissions::KERNEL_DATA`, which resolves to
`PRESENT | WRITABLE | NO_EXECUTE` with `USER_ACCESSIBLE` clear. The common
permission conversion rejects writable+executable mappings. Before the
address-space abstraction is initialized, the kernel validates four-level
paging (`CR4.LA57 == 0`), `EFER.NXE == 1`, and `CR0.WP == 1`; thus NX is
effective and supervisor writes honor read-only mappings. Heap setup does not
change those invariants.

## Interrupt and locking review

`ALLOCATOR` is a `spin::Mutex<HeapAllocator>`. `alloc`, `dealloc`, and
initialization use `without_interrupts` around lock acquisition and mutation.
The architecture helper preserves and restores the prior interrupt state,
including nested calls. A maskable interrupt cannot preempt a same-CPU
allocator critical section and reenter the lock. The allocator's locked
methods only do checked integer arithmetic and fixed-array bitmap operations;
they do not log, format, allocate, or call back into the global allocator.
Invalid-free logging occurs after the lock scope. This is not a claim of an
SMP design; this milestone does not start additional CPUs.

## OOM review

`alloc_error` reports the requested size and alignment through `kprintln!`
and halts in production, or exits the QEMU scenario as failure. Serial output
uses `core::fmt::Arguments`, a static `Once<Mutex<SerialPort>>`, and
`try_lock`; it does not allocate. The handler is called after the allocator
returns null and releases its lock. The dedicated OOM QEMU test directly
calls `GlobalAlloc::alloc` with a request twice the heap size and verifies
null; it does not invoke the language-level `alloc_error_handler`. That
separation is acceptable here because the handler is short, allocation-free,
compiled into every kernel build, and its failure path is deterministic by
inspection. A future test could exercise the handler through a deliberately
failed `Box` allocation under a feature-specific expected-OOM mode.

## Guard-page review

Both guard addresses must translate as unmapped before and after heap
mapping. The lower-guard QEMU test arms an expected-fault matcher, reads the
exact address, and accepts only CPL0 with an empty page-fault error code
(not-present supervisor read, not a write or instruction fetch). Normal boot
does not enable this destructive feature. The upper guard is non-destructively
verified by initialization.

## Frame allocator review

`FrameCursor::next_address` stores a region index and next frame address.
It skips non-usable regions, rounds usable starts up to 4 KiB with checked
addition, rounds exclusive ends down, and advances by one page. Empty and
sub-page regions yield no address. Regions are traversed once in supplied
order; each address in a usable region is returned at most once. The
allocation count increments only after an address is produced and returns
`None` on exhaustion. The diagnostic frame total independently scans regions
and is not on the allocation hot path.

The cursor test verifies alignment, crossing to a second usable region,
skipping a bootloader region and sub-page fragment, overflow handling,
deterministic order, and exhaustion. The actual map is supplied by pinned
bootloader 0.11.17; its API promises usable memory can be freely used and
bootloader-used memory is marked non-usable. The allocator assumes this map
is valid and its usable regions are disjoint; it does not defensively
normalize malformed maps.

## Physical ownership review

Frames returned by `BootFrameAllocator` remain permanently allocated for
this kernel lifetime. Heap backing frames are owned by the heap. `Box`,
`Vec`, and `String` deallocation releases bitmap units only; it does not
return physical frames. If page mapping fails after a frame is consumed,
heap initialization panics/halts and boot does not continue. There is no
unsafe rollback into the monotonic allocator.

## Regression test review

The CI workflow includes executable headless QEMU scenarios for timer boot,
expected page fault, post-unmap fault, supervisor write protection, CPL3
syscall, CPL3 privilege fault, CPL3 supervisor-memory fault, heap success,
heap OOM, and heap guard fault. These are run jobs, not only build matrix
entries. The 0.7 success scenario checks Box value and pointer bounds, 2,048
Vec values after growth, String contents, alignments 1 through 4096,
allocation reuse, fragmented reuse/coalescing, a half-heap allocation, and a
near-capacity allocation bounded by `HEAP_END`.

The requested local rerun from this audited commit passed all ten QEMU
scenarios. `cargo test --locked` passes but the runner has zero host unit
tests; the deterministic QEMU runs are the meaningful execution coverage.

## Release workflow review

Both `.github/workflows/release-candidate.yml` and
`.github/workflows/release.yml` call the reusable CI and Security workflows.
The CI workflow runs all QEMU tests, including the three heap scenarios. The
Security workflow runs cargo audit, cargo deny, actionlint, and custom Rust
CodeQL. Build/package jobs depend on successful validation, CI, and Security,
so a failed heap test or security check prevents packaging and subsequent
publication.

Both workflows validate version inputs and package versions, require a
committed lockfile and changelog entry, verify the dated toolchain, and refuse
to overwrite an existing tag/release. Stable release additionally compares
the operator-confirmed full SHA to `github.sha` and reruns CI/security against
that checked-out commit. At the audited commit, the RC workflow built,
tested, and packaged the selected workflow-dispatch commit and recorded that
SHA in the release manifest and provenance, but did not require that selected
ref to be `main` or ask for a separate SHA confirmation. Commit
`1a92cd557a6b7106022284d48b3641a62e862a91` adds both checks and passes the
validated SHA explicitly to release creation. For RC1, dispatch from `main`
and enter its exact full SHA.

The shared artifact action builds the release ELF and UEFI image, emits a
workspace/Cargo.lock SPDX SBOM (not a binary SBOM), writes `SHA256SUMS` and a
release manifest, then regenerates checksums to include that manifest. The
attestation step covers every file in the artifact directory. Stable publish
is downstream of the `release` GitHub Environment approval job.

The `release` environment exists and has a required reviewer configured.
GitHub reports `can_admins_bypass: true` and `prevent_self_review: false`,
with `ultronstudio` as reviewer. Thus an administrator can bypass the rule,
and the configured reviewer may approve their own run. This is P2
release-process hardening for stable publication, not an RC correctness
failure. If independent approval is intended, disable admin bypass and
require a reviewer distinct from the dispatcher.

## Dependency review

`Cargo.toml` adds only test features; the allocator adds no crate. The lock
delta changes the runner and kernel package versions from 0.0.6 to 0.0.7 and
does not update external dependencies. `cargo deny check` passes with the
existing duplicate-version and unmatched-license-allowance warnings.
`cargo audit --deny warnings --ignore RUSTSEC-2025-0141` passes. The sole
documented exception remains unmaintained `bincode 1.3.3` from the
build-time-only bootloader dependency. No new advisory ignore was added.

## CodeQL history and resolution

The PR development sequence was:

```text
338217a  initial 0.0.7 heap implementation
f12464e  heap bounds/collision validation
3820567  intrusive free-list pointer validation
26e002a  replace intrusive raw-pointer metadata with bounded bitmaps
53dedd3  merge PR #23 into main
```

GitHub annotations on the earlier `3820567` CodeQL run identify five
`Access of invalid pointer` findings in the intrusive list implementation.
On `26e002a`, allocator nodes and their raw-pointer dereferences were
removed; final allocation bookkeeping uses bounded integer indices into
fixed arrays. The final PR CodeQL checks passed. The main-commit Security
workflow at `53dedd3` completed successfully, including `CodeQL (Rust)`.
The code-scanning alerts API returned no open or fixed alert objects for the
repository at audit time; the earlier annotations remain available on the
historical check run and were not dismissed by this audit. A source search
found no `codeql[...]`/`lgtm[...]` suppressions, no CodeQL query exclusion,
and the custom CodeQL workflow and `security-events: write` permission
remain enabled. No security workflow or policy was weakened for the bitmap
redesign.

## Findings

### P0

None.

### P1

None. No valid `GlobalAlloc` usage was found that can overlap allocations,
escape the heap range, access a bitmap out of bounds, or weaken the paging
security boundary. Release CI includes executable heap tests.

### P2

#### PVK-007-AUDIT-001 — RC dispatch did not constrain the selected ref

**ID:** PVK-007-AUDIT-001

**Severity:** P2

**Component:** `.github/workflows/release-candidate.yml`

**Status:** Resolved before RC1 by `1a92cd557a6b7106022284d48b3641a62e862a91`

**Description:** `workflow_dispatch` accepts version and RC number but does
not assert `github.ref` is `refs/heads/main` or compare an operator-entered
SHA.

**Security impact:** CI/security and the release build still operate on the
same selected `github.sha`, and the manifest/attestation identify it. The
remaining risk is accidentally publishing an RC from an unmerged branch.

**Evidence:** RC build uses default checkout, release manifest records
`github.sha`, and `gh release create --target` uses that same SHA; no branch
or commit guard is present in RC validation.

**Resolution:** RC dispatch now requires `refs/heads/main`, validates a
lowercase full 40-character commit SHA against both `github.sha` and the
checked-out `HEAD`, and passes that validated SHA to release creation.

**Release blocking:** Resolved before RC1; hosted workflow checks remain
required before dispatch.

#### PVK-007-AUDIT-002 — deallocation cannot distinguish same-unit Layout sizes

**ID:** PVK-007-AUDIT-002

**Severity:** P2

**Component:** `kernel/src/arch/x86_64/memory/allocator.rs`

**Status:** Open / defensive hardening

**Description:** `deallocate` checks the rounded 16-byte run length but does
not retain the exact original byte size/alignment. Two different sizes that
round to the same unit count can be accepted as equivalent.

**Security impact:** A caller violating the unsafe `GlobalAlloc::dealloc`
contract could free an allocation using a mismatched same-unit layout.
Correct Rust allocator callers use the original layout, so this is not a
failure under valid `GlobalAlloc` use.

**Evidence:** `allocation_units` rounds with `checked_add(15) / 16`; deallocate
compares only `actual_units` to that rounded count and checks the supplied
alignment against the pointer. No per-allocation exact-layout table exists.


**Recommendation:** If rejecting invalid unsafe frees is a required
defense-in-depth guarantee, store and compare exact size/alignment metadata
outside heap objects. Otherwise keep API documentation explicit that only
the `GlobalAlloc` contract is supported.

**Release blocking:** No; invalid caller behavior only.

#### PVK-007-AUDIT-003 — stable Environment approval can be bypassed by admins

**ID:** PVK-007-AUDIT-003

**Severity:** P2

**Component:** GitHub `release` Environment configuration

**Status:** Open / operational policy

**Description:** The environment has a required reviewer, but GitHub reports
`can_admins_bypass: true` and `prevent_self_review: false`.

**Security impact:** An administrator can bypass approval; the configured
reviewer can approve their own deployment. This weakens independent stable
release approval if independence is intended.

**Evidence:** `GET /repos/ultronstudio/pvkernel/environments/release` returned
the required-reviewer rule, `ultronstudio` reviewer, admin bypass enabled,
and self-review prevention disabled.

**Recommendation:** Disable administrator bypass and configure an
independent required reviewer if stable release requires separation of
duties. Confirm the Environment configuration before stable publication.

**Release blocking:** No for RC1; address before stable release if
independent approval is policy.

### P3

#### PVK-007-AUDIT-004 — stale allocator and CI descriptions

**ID:** PVK-007-AUDIT-004

**Severity:** P3

**Component:** `ROADMAP.md`, `docs/development/memory-testing.md`

**Status:** Fixed in this audit branch

**Description:** The roadmap still called the final allocator a free list;
the memory-testing page referenced obsolete CI job names.

**Security impact:** Documentation drift only.

**Evidence:** Direct comparison with final `allocator.rs` and `ci.yml`.

**Recommendation:** Updated both descriptions to bitmap allocation and
current QEMU/Build jobs.

**Release blocking:** No.

#### PVK-007-AUDIT-005 — unexplained `.gitignore` comment

**ID:** PVK-007-AUDIT-005

**Severity:** P3

**Component:** `.gitignore`

**Status:** Open / cosmetic

**Description:** `# DON'T DELETE!!!` above `release-test/` does not explain
the ignored directory.

**Security impact:** None identified.

**Evidence:** Merged PR #23 `.gitignore` change.

**Recommendation:** In a separate housekeeping change, replace the comment
with `# Local release smoke-test artifacts`.

**Release blocking:** No.

### Informational

#### PVK-007-AUDIT-006 — OOM test exercises null return, not the handler

**ID:** PVK-007-AUDIT-006

**Severity:** INFO

**Component:** OOM test coverage

**Status:** Accepted for 0.0.7

**Description:** The expected-OOM QEMU test calls `GlobalAlloc::alloc` and
checks for null rather than invoking `alloc_error_handler`.

**Security impact:** The allocator's failure result is exercised; handler
reporting/halt is verified by code inspection and compilation, not a runtime
scenario. The handler is allocation-free and runs after the allocator lock
is released.

**Evidence:** `heap::run_oom_test`, `main.rs::alloc_error`, and serial
`kprintln!` implementation.

**Recommendation:** Consider an expected-handler QEMU mode in a later test
hardening pass; no 0.0.7 blocker identified.

**Release blocking:** No.

#### PVK-007-AUDIT-007 — only the lower guard is deliberately faulted

**ID:** PVK-007-AUDIT-007

**Severity:** INFO

**Component:** Guard-page test coverage

**Status:** Accepted for 0.0.7

**Description:** QEMU deliberately accesses the lower guard only.

**Security impact:** Both lower and upper guards are checked unmapped before
and after mapping; the heap mapping loop cannot map either guard. The missing
upper destructive test does not leave an unverified mapping invariant.

**Evidence:** `heap::init` guard translations and `run_guard_fault_test`.

**Recommendation:** No change required for this fixed uniform mapping.

**Release blocking:** No.

## Known accepted limitations

* Fixed 1 MiB heap; no dynamic growth.
* Static 16 KiB bitmap metadata and 16-byte allocation granularity; small
  allocations can leave up to 15 bytes of internal slack per allocation.
* First-fit search is linear in the units inspected.
* No general physical-frame reclamation; heap backing frames remain owned
  for kernel lifetime.
* No per-process heap or slab allocator.
* Locking is appropriate for the current single-CPU kernel; SMP lifecycle
  and allocator behavior are not part of this milestone.
* Boot correctness relies on the bootloader's valid, disjoint usable-memory
  map contract.

## Validation performed

| Check | Result |
|---|---|
| Exact target and clean audit branch from `origin/main` | Passed; `53dedd38b43e7aa12a4175390a9af2dc9ccbb3a1` |
| Full `v0.0.6..origin/main` delta and commit history reviewed | Passed; 17 changed files |
| `cargo fmt --all -- --check` | Passed |
| Kernel Clippy, locked, `x86_64-unknown-none`, warnings denied | Passed |
| Runner Clippy, locked, warnings denied | Passed |
| `cargo test --locked` | Passed; runner contains 0 host unit tests |
| `cargo deny check` | Passed; existing duplicate/version and unmatched-license warnings only |
| `cargo audit --deny warnings --ignore RUSTSEC-2025-0141` | Passed; documented exception only |
| `actionlint` | Passed |
| `python -I scripts/check_security_policy.py .` | Passed |
| Python unittest discovery | 65 passed; one known Windows-invalid `::error::x.yml` fixture errored with `OSError: [Errno 22] Invalid argument` |
| All ten deterministic QEMU scenarios | Passed locally on the audited commit |
| `cargo build --release --locked` | Passed |
| Final CodeQL (Rust) on main merge commit | Passed in Security workflow run `40079386178` |
| Main-commit cargo audit, cargo deny, actionlint | Passed in Security workflow run `40079386178` |
| Code-scanning alert API | No current open alerts; historical intrusive-list annotations inspected on check run `120826428857` |
| `git diff --check` | Passed after the audit document and documentation edits |

## Release recommendation

**CONDITIONALLY READY at the audited commit — no P0/P1 findings; three P2
follow-ups were recorded.**

The RC dispatch follow-up was resolved in commit
`1a92cd557a6b7106022284d48b3641a62e862a91`; two P2 follow-ups remain: exact
invalid-layout detection and the stable Environment approval policy. After
human review and merge of the hardening PR, RC1 can be dispatched from `main`
with its exact SHA. The RC workflow will test, package, and attest that commit.
No finding here authorizes stable publication.
