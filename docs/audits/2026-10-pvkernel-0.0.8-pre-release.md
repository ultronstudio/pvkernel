# pvKernel 0.0.8 Pre-release Audit

**Audit date:** 2026-10-24
**Audited `origin/main`:** `15a1c09c1b96b0368acabcac558dec8364622d8f`
**Baseline:** `v0.0.7`, `1621819015def0b1cca8d7b702eab1831e5a67b8`
**Implementation:** PR #26, rebased scheduler/task implementation

## Scope

This audit reviews the complete `v0.0.7..15a1c09c1b96b0368acabcac558dec8364622d8f`
delta: 24 changed files covering bounded ring-0 tasks, guarded stacks, the
IRQ0 context-switch boundary, QEMU coverage, documentation, and CI integration.
It does not implement or audit the future 0.0.9 process/address-space work.

The audit branch was created from the exact merged commit after fetching and
fast-forwarding `main`. At audit start, local `main` and `origin/main` both
resolved to the target SHA and the worktree was clean. This branch contains
only this audit report.

The implementation PR's old head, `dc35e397cd5005006cf806f0b569dbfb96918f4e`,
was not used as the audit target. The rebased merge result was used throughout.

## Audited commit

```text
main:        15a1c09c1b96b0368acabcac558dec8364622d8f
origin/main: 15a1c09c1b96b0368acabcac558dec8364622d8f
base:        1621819015def0b1cca8d7b702eab1831e5a67b8 (v0.0.7)
```

The delta contains four focused commits: design documentation, scheduler and
context switching, deterministic scheduler QEMU scenarios, and final scheduler
documentation. No 0.0.8 or 0.0.8-rc.1 tag or release exists.

## Executive summary

No P0 or P1 correctness/security blocker was found in the audited 0.0.8 state.
The complete software context layout matches the IRQ assembly; Intel's 64-bit
interrupt-frame semantics match the five-word hardware/synthetic frame; CPL3
timer returns bypass task scheduling; stacks are bounded, supervisor-only,
RW/NX, and guarded; and task exit cannot make its own active stack reusable.
The target configuration and a compiler-generated floating-point probe confirm
that Rust floating-point operations use software helpers rather than task-live
FPU/SIMD registers. Scheduler and existing memory/heap locks prevent same-CPU
preemption deadlocks.

All 14 QEMU scenarios, the native Windows graphical boot, all requested Rust,
dependency, workflow, and policy checks, and the exact-main CI/Security runs
passed. The Python suite had one known native-Windows error because the
pre-existing hostile-filename fixture contains `::`, which Windows rejects.
The suite was not weakened; the exact-main security workflow and policy
validator passed.

One P3 documentation issue remains: `CHANGELOG.md` still labels the already
stable 0.0.7 section “Unreleased”. This does not affect RC1 correctness or the
new 0.0.8 entry, which correctly remains Unreleased. One informational
platform-specific Python test limitation is also recorded. Neither blocks RC1.

**Recommendation: READY FOR RC1.** This is only an RC1 readiness decision. It
does not approve stable publication. This audit does not create RC1.

## Architecture changes since 0.0.7

The scheduler adds eight fixed task slots, generation-aware IDs, eight
pre-mapped 16 KiB task stacks, two unmapped guard pages per stack slot, a fixed
ring ready queue, and one-tick round-robin decisions from PIT IRQ0. A dedicated
assembly entry saves the GPRs and long-mode return frame, calls a small Rust
policy function, then restores the selected context with `iretq`. CPL3-origin
timer interrupts record ticks and EOI but return to the same userspace frame.

The scheduler shares the existing kernel CR3 and remains single-core. Task
stack frames are retained by the task-stack subsystem for kernel lifetime;
there is no general physical-frame reclamation. The implementation adds no
third-party scheduler/runtime dependency and changes no release workflow.

## Threat model

**Security boundary.** Kernel tasks execute at CPL0 in the shared kernel
address space. Their stacks are supervisor-only and NX, but tasks are not
isolated from one another or from the kernel. This milestone adds no
userspace scheduling or process boundary.

**Trusted internal API assumption.** `spawn_kernel_task` accepts a kernel
function pointer supplied by trusted kernel code. It does not accept user
input, arbitrary virtual addresses, closures, or userspace entry points.
Kernel task code and scheduler metadata are trusted; corrupted metadata is a
kernel integrity failure and is handled as a fatal invariant violation.

**Execution model.** One CPU receives maskable PIT preemption at an arbitrary
kernel instruction boundary. Interrupt-gate IF clearing prevents nested
maskable scheduler entry. Same-CPU spin locks must either mask interrupts
while held or use a nonblocking operation. The audit covers stale handles,
queue integrity, stack overflow/overlap/reuse, malformed synthetic contexts,
context restore, and scheduler reentrancy.

**Future boundary.** A shared CR3, trusted task entry, and kernel-fatal panic
are accepted 0.0.8 limitations. Userspace processes, independent address
spaces, CR3 switching, and isolation are future 0.0.9 work and are not implied
by this audit.

## Scheduler/task model review

`MAX_KERNEL_TASKS` is 8. Scheduler state and the runnable ring are fixed-size
arrays; the timer path performs no allocation or unbounded scan. `TaskSlot`
stores generation, state, saved context pointer, and queue membership; the
slot index supplies the stack-slot identity. Exactly one ordinary task is
`Running`, or the scheduler tracks the special bootstrap context. The queue
contains only `Ready` tasks with matching generations and `queued = true`.

Task-management APIs mask interrupts before taking the scheduler mutex.
IRQ0 arrives through an interrupt gate, so the scheduler callback begins with
IF clear. `without_interrupts` preserves the old IF state, including when
nested in IRQ context. The scheduler mutex scope ends before the selected
context pointer is returned to assembly.

## TaskId and lifecycle review

`TaskId` is a private-field `(u8 slot, u64 generation)` value, not a pointer.
The eight-slot bound is checked when converting a slot to `u8`; lookup uses
slice `.get()` and validates the generation. Initial generation is 1. Reuse
uses `checked_add`; a slot at `u64::MAX` becomes `Retired`, so it cannot wrap
and alias an ancient handle. Stale IDs return `None` from state lookup.

The observed lifecycle is:

```text
Vacant → Ready → Running → Ready (timer preemption)
                         → Exiting → Exited (after switch-away)
Exited → Ready (reuse with incremented generation)
generation exhaustion → Retired
```

The scheduler checks `Running`/`Exiting` for the current task, verifies its
generation and queue flag, and checks that dequeued entries match both their
generation and `Ready` state. Exited, vacant, and retired tasks are not
requeued. Invariant violations panic; normal table exhaustion returns
`TaskTableFull`.

## Stack virtual-memory review

The reserved region is `0xffff900020000000..0xffff900020030000` (192 KiB).
Each 24 KiB slot is one lower unmapped 4 KiB guard, four mapped pages (16 KiB),
and one upper unmapped 4 KiB guard. The mapped low address is one page above
the slot base; the initial stack top is the exclusive high end of its mapped
pages. The stack grows downward toward the lower guard. Adjacent slots have
distinct guard pages; no guard page is also a mapped stack page.

All address arithmetic uses checked operations, including region end, slot
stride/offset, stack bounds, and guard addresses. The subsystem checks the
entire region for existing translations before mapping any stack, then checks
each slot's first, middle, and final mapped page and both guards after mapping.
This runtime check covers dynamic bootloader mappings as well as the known
kernel image, boot stack, physical map, framebuffer, heap/guards, and temporary
mapping test range. A collision or mapping failure is fatal before scheduler
activation.

Stack mapping uses the central `Permissions::KERNEL_DATA` abstraction. The
page-table permission conversion enforces W^X; post-map translations confirm
`PRESENT | WRITABLE | NO_EXECUTE` and clear `USER_ACCESSIBLE`. Existing paging
boot checks keep NXE and CR0.WP enabled and reject LA57. No task stack is
executable or user accessible.

All slots are pre-mapped before IRQ0 is unmasked. Stack frames remain owned by
the task-stack subsystem for kernel lifetime, and are never returned to the
monotonic boot frame allocator. Slot reuse clears exactly the 16 KiB mapped
stack range, not either guard, after the prior execution has switched away.

## Context-frame review

`Context` is `#[repr(C)]`, exactly 20 `u64` words. Compile-time size and
`offset_of!` assertions pin the hardware-frame offsets. IRQ0 has no error
code. Assembly pushes, in order:

```text
rax, rbx, rcx, rdx, rbp, rsi, rdi, r8, r9, r10, r11, r12, r13, r14, r15
```

Since the stack grows downward, the resulting memory order at the saved
pointer is:

```text
r15 r14 r13 r12 r11 r10 r9 r8 rdi rsi rbp rdx rcx rbx rax
RIP CS RFLAGS RSP SS
```

This matches `Context` field-by-field. Restore pops the reverse push order,
then `iretq` consumes the five hardware/synthetic words. This preserves all
15 required GPRs, including caller-saved registers, rather than only ABI
callee-saved registers. The callback result is returned in RAX and adopted as
the new context pointer after Rust returns; the old pointer is passed in RDI.

## IRQ0 assembly/context-switch review

`switch.rs` is the only new scheduler assembly entry. IRQ0 is registered at
vector 32 with raw `set_handler_addr`; other interrupt handlers remain Rust
`x86-interrupt` handlers. The saved context pointer remains on the interrupted
stack while assembly rounds the temporary Rust-call stack down to 16-byte
alignment. The call return address and callback frames grow below the saved
context; they do not overwrite or alias the saved register/frame area. The
callback returns a scheduler-owned saved frame pointer. The scheduler lock
has been dropped before assembly assigns it to RSP.

The function-pointer trampoline begins with the synthetic task frame already
restored by the same pop/`iretq` path. Its RSP is 16-byte aligned; `call rdi`
pushes a return address so the Rust `fn()` entry observes the SysV-required
8-mod-16 entry alignment. When the entry returns, the trampoline calls the
nonreturning task-exit function. It cannot return into an uninitialized stack
address.

The Intel SDM independently confirms this frame assumption: in 64-bit mode,
interrupt/exception entry pushes `SS:RSP` unconditionally, including without
a CPL change, and `iretq` consumes the 64-bit frame. See Intel SDM Vol. 3A,
§7.14, “Exception and Interrupt Handling in 64-Bit Mode”, especially the
64-bit stack-frame description: [Intel SDM PDF](https://cdrdv2-public.intel.com/835754/253668-sdm-vol-3a.pdf).
Thus both CPL0→CPL0 and CPL3→CPL0 entries have `RIP, CS, RFLAGS, RSP, SS` in
the supported 64-bit mode. CPL3 entry additionally changes the active stack
to TSS.RSP0 before pushing the frame. The actual configured IDT gate is an
interrupt gate, so IF is cleared on entry.

## CPL0/CPL3 interrupt review

`timer_schedule` increments the PIT tick once, inspects the saved `CS.RPL`,
and immediately returns the same context for CPL3-origin IRQs after the
userspace-test observation and one EOI. Assembly then restores all saved GPRs
and the five-word userspace frame through `iretq`, returning to the same
payload. It does not take the scheduler mutex or select a kernel task on that
path.

Existing CPL3 code uses the unchanged TSS.RSP0 for CPL3→CPL0 transitions. The
`int 0x80` entry and syscall ABI are unchanged. When a PIT interrupt occurs
while the CPU is executing CPL0 inside the syscall dispatcher, it is correctly
classified by the actual interrupted CS as CPL0 and may schedule; the saved
bootstrap/syscall context later resumes on its own stack. Existing CPL3
syscall, privileged-instruction, and supervisor-memory scenarios passed.

## Stack alignment / ABI review

For timer entry, the interrupt frame and 15 GPR pushes remain below the
interrupted RSP. `and rsp, -16` only rounds the temporary call stack downward;
the context pointer is preserved in RDI and is not lost. The Rust `extern
"C"` callback uses the target's SysV x86_64 call convention. The callback
return address is below, not inside, the saved frame. The target disables the
red zone, so asynchronous entry does not invalidate compiler-owned red-zone
locals.

For new tasks, the synthetic frame is placed at `stack_top - 160`, with
checked subtraction and alignment validation. `iretq` loads RSP from the
synthetic hardware frame as the aligned stack top. The trampoline aligns before
its call, resulting in the correct function-entry alignment described above.

## RFLAGS / DF / selector review

Synthetic `RFLAGS = 0x202`: bit 1 is set, IF is set, IOPL is zero, DF is clear,
and TF is clear. No current flags are inherited. The IRQ gate clears IF during
bookkeeping; no scheduler path executes `sti`; IF becomes active only when
`iretq` restores the selected task/bootstrap RFLAGS.

Assembly executes `cld` before Rust, satisfying the ABI's clear-DF invariant.
The original task DF is still in its hardware-saved RFLAGS and is restored by
`iretq`; task entry starts with DF clear. The kernel code selector comes from
`gdt::kernel_code_selector()` after GDT initialization, not from a magic
selector. GDT initialization sets SS to the null selector in long mode; the
synthetic SS is the same intended null kernel selector. CPL0 IRETQ to this
kernel context is accepted by the architecture and exercised by QEMU.

## Timer preemption review

IRQ0 uses the existing approximately 99 Hz PIT and a one-tick quantum. The
tick counter is not reset by scheduler initialization. Each callback records
one tick before scheduling. A currently running task that remains runnable
becomes `Ready` and is enqueued once; the next ready task is selected. If no
ordinary task is ready, bootstrap is restored. No switch is needed when a
task is the only runnable work.

The scheduler IRQ path contains no heap allocation, `Box`, `Vec`, `String`,
formatting, ordinary logging, page/stack mapping, blocking, or unbounded scan.
Normal work is fixed-size state updates plus one ring pop/push and invariant
checks. Fatal invariant paths may panic/log by design; they are not ordinary
IRQ operation.

The timer-driven test has two CPU-bound tasks with no yield. A cannot finish
until B increments progress; B cannot finish until A reaches a threshold.
Relaxed atomics are sufficient for independent progress counters/flags because
there is one CPU and they do not publish non-atomic payload data. A tick-based
watchdog makes scheduler failure bounded. It logs/exits from task context, not
the IRQ hot path.

## Task exit and reuse review

Returning from a task marks it `Exiting` with interrupts masked, then uses
`sti; hlt` until IRQ0 arrives. STI's one-instruction interrupt shadow makes
the following HLT execute before a pending maskable interrupt is recognized;
an already pending timer wakes the HLT and is then serviced, avoiding a
lost-wakeup window. At boot, IRQ0 is the only unmasked hardware line.
Unrelated maskable handlers do not invoke scheduling.

The PIT callback saves the exiting task's complete context pointer and changes
its state to `Exited`, then selects a different ready task or bootstrap. There
is a short interval where metadata says `Exited` before assembly executes
`mov rsp, rax`; during that interval the CPU is still in the IRQ entry, IF is
clear, and no ordinary task-management API can run. The callback releases the
scheduler lock before returning. Assembly then adopts the other context's RSP
and restores it. Only after that switch can ordinary task code run and reuse
the exited slot. The old exiting task is never enqueued, so its saved context
cannot resume. This establishes the single-core proof that an active stack is
not cleared or reused.

The kernel uses abort-style panic semantics; task panic is kernel-fatal. There
is no task-local panic recovery, join, or blocking lifecycle.

## Runnable queue review

The ready queue is a fixed ring with `head`, `len`, and capacity 8. Tail is
`(head + len) % MAX_KERNEL_TASKS`; both operands are at most 7, so the sum is
at most 14 and cannot overflow `usize`. Push checks full and an empty tail;
pop checks empty, takes one entry, advances head modulo capacity, and reduces
length. The initialization self-test explicitly fills, detects full, dequeues,
wraps, and checks order. Its synthetic IDs with indices beyond the live task
table are only opaque queue values and are never dereferenced.

Queue uniqueness is O(1) through per-slot `queued` state and transition
assertions: spawn requires an unqueued reusable slot; preemption requires the
Running task not queued before marking/enqueueing it; dequeue verifies the
matching generation, `Ready` state, and queued bit. In healthy state, a full
queue implies all 8 slots are ready and no spawnable slot exists.

## Spawn atomicity review

Spawn disables interrupts, holds the scheduler lock, selects a `Vacant` or
`Exited` slot, advances its generation with checked arithmetic, clears the
inactive stack, writes the complete synthetic context, pushes the ID, then
publishes the `Ready` slot. No observer can run between queue insertion and
slot publication because IF is clear and the lock is held; after a successful
push, the slot assignment is infallible. If push reports `QueueFull`, it has
not mutated the queue and the slot metadata remains `Vacant`/`Exited`; only
that inactive stack's bytes/context were overwritten. Neither is runnable,
and a later spawn synthesizes it again. The old exited context is not a join
resource and cannot be resumed. This is safe for the current API, which has
no join or saved-context access. The table-full path performs no stack writes.

Generation-exhausted slots are retired while searching. This is an intentional
permanent transition even if no other slot is available. Normal capacity
exhaustion remains a deterministic error, not a panic.

## Locking and preemption review

* Scheduler mutex: all ordinary API access is inside `without_interrupts`; IRQ
  entry already has IF clear. The lock is released before the context switch.
* Heap allocator: `alloc`, `dealloc`, and initialization mask interrupts around
  the heap mutex. A timer cannot switch away while a task owns it.
* Page-table/address-space state: `with_kernel_address_space` masks interrupts
  around the memory mutex. Stack mapping only happens during initialization,
  before timer unmasking.
* Serial: `_print` uses `try_lock`, never spins. If a task is preempted while
  serial is held, another task's print is skipped instead of deadlocking; this
  accepted behavior can lose diagnostic output under contention. Timer IRQ
  scheduling does not log.
* Framebuffer: rendering occurs during boot before IF is enabled; no scheduler
  task or timer hot path takes a framebuffer lock.
* PIC/PIT: port programming and masking happen during boot with interrupts
  disabled; PIT tick state is atomic. EOI is the timer entry's only normal
  IRQ-side PIC access. The design is single-core and does not claim SMP safety.

No audited ordinary task path can be switched away while holding an existing
scheduler-unsafe spin lock. Future kernel locks must follow the documented
interrupt-masking rule.

## PIC EOI review

IRQ0 records exactly one tick. Both CPL3 and CPL0 paths call the PIC EOI helper
once. The CPL3 path acknowledges before returning its unchanged context. The
CPL0 path acknowledges once after the scheduling decision and lock release,
before the assembly restore. IRQ0 is a master-PIC line, so the helper writes
one master EOI and no slave EOI. This is true whether the context is task,
bootstrap, switched, or not switched, including task exit.

## FPU/SIMD and red-zone review

The exact toolchain is pinned to `nightly-2026-10-23` (rustc commit
`7189d39d9ce936881b8132c6addeaf3e295cd455`). Its actual
`x86_64-unknown-none` target specification reports:

```text
disable-redzone: true
features: -mmx,-sse,-sse2,-sse3,-ssse3,-sse4.1,-sse4.2,-avx,-avx2,+soft-float
rustc-abi: softfloat
```

The target also reports `x87`/`fxsr` as target feature cfg names, so their
presence was not ignored. A small `f64` addition probe compiled with the exact
target/toolchain emitted a call to the software helper `__adddf3` and no x87,
MMX, SSE, or AVX instructions. The kernel source has no floating-point or
SIMD intrinsic use. The kernel build script rejects the exact
`x86_64-unknown-none` mismatch and rejects enabled MMX/SSE/AVX feature names.
Thus compiler-generated task-visible FP/SIMD state is excluded for this pinned
build. Explicit future inline assembly or target-feature code using FPU/SIMD
would violate the trusted kernel-task policy and must add eager save/restore
before use. The target's disabled red zone is verified independently of
QEMU.

## TSS/RSP0 compatibility review

Task stacks are separate virtual mappings and are never installed in
TSS.RSP0 or the double-fault IST entry. TSS.RSP0 remains the existing dedicated
ring-transition stack for CPL3→CPL0 entry; the double-fault IST remains
separate. CPL3 timer returns use the RSP0-created frame without switching
kernel tasks. Existing syscall/fault paths passed their QEMU scenarios.

## Physical ownership review

The scheduler pre-maps 32 stack pages total. Their physical frames are
monotonically allocated and remain task-stack-subsystem-owned for kernel
lifetime. Reusing a task slot clears/reuses the same mapped backing only after
the old task is inactive. No frame deallocator or general reclamation was
added, and the documentation makes no physical-reclamation claim. The
existing heap same-unit invalid-layout hardening limitation remains the
previously accepted P2 for invalid unsafe allocator callers; 0.0.8 did not
change that risk.

## Regression test review

Normal boot creates two finite tasks on separate stacks, checks their stack
pointers and disjoint ranges, waits for both to become `Exited`, and confirms
bootstrap resumes. The preemption test does not use voluntary yield. The
capacity test fills all slots, expects a ninth spawn to return
`TaskTableFull`, waits for exits, reuses a slot, checks generation change and
stale-ID rejection, then waits for replacement exit. The stack-guard test
reads the current task's lower guard and validates CR2, CPL0, non-present,
read, supervisor, and non-instruction-fetch bits before reporting expected
success.

The expected-fault checks are feature-gated and do not run in normal boot.
All old CPL3 scenarios remain in CI and were executed during this audit.

## QEMU test results

All 14 scenarios were executed locally on the exact audited checkout using
the repository runner, `--locked`, and headless `isa-debug-exit`:

| Scenario | Result |
|---|---|
| Timer boot | PASS |
| Expected page fault | PASS |
| Post-unmap page fault | PASS |
| Supervisor write protection | PASS |
| CPL3 syscall boundary/timer observation | PASS |
| CPL3 privileged-instruction fault | PASS |
| CPL3 supervisor-memory fault | PASS |
| Kernel heap allocation | PASS |
| Kernel heap OOM | PASS |
| Kernel heap guard page | PASS |
| Kernel scheduler smoke | PASS |
| Timer-driven task preemption | PASS |
| Task capacity/reuse/stale ID | PASS |
| Task stack guard page | PASS |

The native Windows runner used QEMU from a process-local PATH. No persistent
system PATH change was made.

## Native Windows graphical boot

`cargo run --locked` launched graphical QEMU successfully on native Windows.
The boot reached both scheduler smoke completion and:

```text
pvKernel 0.0.8 initialized
entering interrupt-enabled idle loop
```

No panic, unexpected page/general-protection fault, reboot, triple fault,
hang-before-idle, or GTK startup failure was observed. QEMU was stopped after
confirming stable idle.

## CI and release-workflow review

The changed CI workflow adds four real QEMU execution jobs: scheduler smoke,
timer-driven preemption, task stack guard, and task capacity/reuse. They use
the existing QEMU scenario action, which builds and executes headless QEMU;
they are not compile-only jobs. The old timer, memory, heap, and CPL3 scenarios
remain present.

On the exact merged SHA, GitHub `CI`, `Security`, and `Security triage` runs
completed successfully (runs `40090745201`, `40090745163`, and `40090745266`).
The custom Rust CodeQL job was successful. No release or security workflow was
changed by the 0.0.8 delta.

The RC workflow still requires dispatch from `refs/heads/main`, a full
lowercase SHA equal to both `github.sha` and checked-out `HEAD`, successful
reusable correctness and security gates, exact pinned toolchain/version,
changelog/lock checks, and a previously unused tag. The artifact workflow
continues to generate ELF/UEFI artifacts, SHA256SUMS, release manifest, SPDX
SBOM, and provenance attestation. Stable publication still requires the
`release` GitHub Environment approval job and validates the exact selected
commit. No permission broadening or release-gate bypass was introduced.

Live `release` Environment settings were inspected: required reviewer is
`ultronstudio`; admin bypass is off; the custom deployment branch policy
permits only `main`; `prevent_self_review` is off. Self-review prevention is
intentionally not recommended for this solo-maintainer repository. The
environment exists, so the workflow's warning about an absent environment
does not apply.

## Dependency review

The `Cargo.lock` delta only updates the `kernel` and runner package versions
from 0.0.7 to 0.0.8. No external crate version changed and no scheduler,
async-runtime, or other runtime dependency was added. The accepted
`RUSTSEC-2025-0141` ignore remains narrow and documented for build-time
`bincode 1.3.3`; no new ignore was added.

## Security tooling / CodeQL review

The custom Advanced CodeQL workflow remains configured and passed on the exact
main commit. GitHub reported zero open code-scanning alerts at audit time. No
default CodeQL workflow was enabled, no query/workflow weakening or new
suppressions were found, and no new security finding from the scheduler was
surfaced.

`cargo audit --deny warnings --ignore RUSTSEC-2025-0141`, `cargo deny check`,
`actionlint`, and the repository security-policy validator passed. Cargo deny
reported existing duplicate-version/unmatched-license warnings but exited
successfully under the repository policy.

## Documentation review

The changed README, roadmap, architecture overview, changelog, boot,
interrupt, timer, virtual-memory, scheduler, and testing docs describe the
bounded ring-0 scheduler and current limitations consistently. They do not
claim process scheduling, per-process CR3, VFS, SMP/APIC, general frame
reclamation, stable userspace ABI, or a released 0.0.8. The 0.0.8 changelog
entry is explicitly `Unreleased`.

`docs/architecture/userspace-entry.md` still says its **0.0.6** userspace
milestone has no scheduler. That statement is scoped to the 0.0.6 feature and
does not contradict the later 0.0.8 kernel-task milestone.

## Findings

### P0

None.

### P1

None.

### P2

None found that requires resolution before RC1. The previously accepted heap
same-unit invalid-layout limitation applies only to callers violating the
unsafe `GlobalAlloc::dealloc` contract and is unchanged by this delta.

### P3

#### PVK-008-AUDIT-001 — historical 0.0.7 changelog status is stale

**Severity:** P3
**Area:** Documentation
**Status:** Open, documentation-only
**Evidence:** `CHANGELOG.md` adds the correct `[0.0.8] - Unreleased` section,
but the following historical `[0.0.7] - Unreleased` section remains although
v0.0.7 is a published stable release. This label predates the audited delta.
**Impact:** Release-history status is inaccurate; no kernel or artifact
integrity impact.
**Required action:** Correct the historical changelog status in a separate
documentation change before final 0.0.8 stable notes are finalized.
**RC1 blocker:** No.

### INFO

#### PVK-008-AUDIT-002 — native Windows cannot create the hostile-name fixture

**Severity:** INFO
**Area:** Python security-policy tests
**Status:** Accepted platform limitation
**Evidence:** Python unittest discovery ran 66 tests: 65 passed; the existing
`test_hostile_filename_is_sanitized` errored when Windows rejected the fixture
path containing `::` with `OSError: [Errno 22] Invalid argument`. The fixture
was not changed or skipped. Exact-main Security and policy validation passed
in CI.
**Impact:** That one fixture cannot run on native Windows; CI remains its
authoritative host for the case.
**Required action:** None for RC1; retain the fixture and its CI coverage.
**RC1 blocker:** No.

## Required findings review

| # | Review item | Result |
|---:|---|---|
| 1 | Context save/restore ordering | PASS — assembly push/pop and `Context` order match. |
| 2 | Five-word long-mode interrupt frame | PASS — Intel SDM §7.14 confirms unconditional SS:RSP push in 64-bit mode. |
| 3 | Synthetic kernel context | PASS — complete initialized frame; checked stack placement. |
| 4 | Stack call alignment | PASS — timer callback and task entry satisfy SysV alignment. |
| 5 | DF handling | PASS — `cld` before Rust; original DF restored via RFLAGS. |
| 6 | RFLAGS policy | PASS — `0x202` sets bit 1/IF and clears IOPL/DF/TF. |
| 7 | Kernel CS/SS | PASS — actual GDT code selector; intended null kernel SS matches loaded SS. |
| 8 | CPL3 timer preservation | PASS — same context returned, tick and one EOI, no task scheduling. |
| 9 | Bootstrap context | PASS — special non-queued context captured before selection; refreshed on idle ticks. |
| 10 | Two-phase task exit/reuse | PASS — slot becomes reusable only after switch-away under IF-cleared IRQ path. |
| 11 | `sti; hlt` exit loop | PASS — STI shadow plus HLT avoids a pending-timer lost wakeup; only IRQ0 is unmasked. |
| 12 | TaskId generation exhaustion | PASS — checked increment, generation starts at 1, exhausted slot retired. |
| 13 | Ready-ring integrity | PASS — bounded indices, O(1) flags/state checks, explicit wrap/full/empty test. |
| 14 | Spawn failure atomicity | PASS — no runnable publication before queue success; failed queue push leaves metadata non-runnable. |
| 15 | Stack virtual range | PASS — checked region arithmetic and runtime collision detection. |
| 16 | Guard-page isolation | PASS — both guards distinct/unmapped; destructive lower-guard QEMU check validates fault class. |
| 17 | RW/NX/supervisor stack permissions | PASS — actual translations checked; W^X/NXE/WP policy retained. |
| 18 | Physical frame ownership | PASS — stack backing retained for kernel lifetime; no reclamation claim. |
| 19 | Scheduler IRQ allocation-free | PASS — fixed arrays and bounded operations only. |
| 20 | Scheduler IRQ logging-free | PASS — normal timer path has no logging. |
| 21 | Scheduler lock/preemption behavior | PASS — task APIs mask interrupts; lock released before switch. |
| 22 | Serial `try_lock` under preemption | PASS — contending task drops output rather than spinning; logs may be lost. |
| 23 | Heap lock under preemption | PASS — allocator masks interrupts around its mutex. |
| 24 | Page-table lock under preemption | PASS — address-space API masks interrupts around its mutex. |
| 25 | Timer EOI exactly once | PASS — one master EOI on CPL0 and CPL3 branches. |
| 26 | Timer tick monotonicity | PASS — one atomic increment/IRQ, never reset by scheduler; observed across tests. |
| 27 | FPU/SIMD exclusion | PASS — pinned soft-float target; compiled `f64` probe calls `__adddf3`, no FP/SIMD instructions. |
| 28 | Red-zone disabled | PASS — pinned target spec reports `disable-redzone: true`. |
| 29 | TSS/RSP0 compatibility | PASS — task stacks do not replace RSP0 or double-fault IST. |
| 30 | CPL3 syscall/fault regressions | PASS — all three existing CPL3 QEMU tests passed. |
| 31 | All fourteen QEMU scenarios | PASS — each listed scenario was actually executed. |
| 32 | Graphical Windows boot | PASS — scheduler smoke reached stable idle with no unexpected fault. |
| 33 | CodeQL | PASS — custom Rust CodeQL passed; zero open alerts. |
| 34 | Dependency/security tooling | PASS — no new dependency; audit, deny, actionlint, policy checks passed. |
| 35 | RC exact-SHA workflow | PASS — main ref, full SHA, checkout SHA, CI/security gates, and tag checks remain. |
| 36 | Stable release environment | PASS — required reviewer, admin bypass off, `main` only; solo-maintainer self-review setting retained. |
| 37 | Documentation consistency | P3 — 0.0.7 historical changelog status is stale; 0.0.8 remains Unreleased and current scheduler scope is accurate. |

## Validation performed

| Check | Result |
|---|---|
| Exact target and clean audit branch | Passed; target `15a1c09c1b96b0368acabcac558dec8364622d8f` |
| Full `v0.0.7..target` delta enumerated/reviewed | Passed; 24 changed files |
| `cargo fmt --all -- --check` | Passed |
| Kernel Clippy, locked, `x86_64-unknown-none`, warnings denied | Passed |
| Runner Clippy, locked, warnings denied | Passed |
| `cargo test --locked` | Passed; runner has 0 host unit tests |
| `cargo deny check` | Passed; existing duplicate/license warnings only |
| `cargo audit --deny warnings --ignore RUSTSEC-2025-0141` | Passed; existing accepted advisory exception only |
| `actionlint` | Passed |
| `python -I scripts/check_security_policy.py .` | Passed |
| Python unittest discovery | 65 passed; one pre-existing Windows-invalid `::` filename fixture errored |
| 14 old/new deterministic QEMU scenarios | Passed locally on the exact audited commit |
| Native Windows graphical QEMU boot | Passed to scheduler smoke and stable idle |
| `cargo build --locked --release` | Passed |
| `git diff --check` before audit document | Passed; rerun after document below |
| Exact-main CI/Security/Security triage | Passed; runs `40090745201`, `40090745163`, `40090745266` |
| Open code-scanning alerts | Zero at audit time |
| Release Environment | Present; `ultronstudio`, no admin bypass, branch `main`, self-review prevention off |
| 0.0.8/0.0.8-rc.1 tags and releases | None |

## RC1 readiness decision

**READY FOR RC1.** No unresolved P0 or P1 exists. The P3 changelog status and
INFO Windows fixture limitation do not block RC1. This recommendation permits
only a separately reviewed RC1 dispatch from the exact then-current `main`
SHA after this audit document is reviewed and merged. It does not approve or
publish a stable release and does not create an RC.

## Deferred work

0.0.9 remains process lifecycle, per-process address spaces/CR3 switching, and
userspace ELF foundation. 0.0.10 remains VFS/initrd/root-filesystem
foundation. APIC, SMP, multicore scheduling, blocking/wait queues, sleep,
priorities, joins, userspace scheduling, and general frame reclamation remain
later work.
