# pvKernel 0.0.9 pre-release audit

**Verdict: READY FOR RC1 REVIEW**

This report records a formal pre-release source audit of pvKernel 0.0.9,
“Processes + Address Spaces + Userspace Foundation.” The audit subject is the
exact merged implementation commit
`8370457139e1530467cc13ba7308b56eaf802eaf`. Later commits on the audit
documentation branch are not part of the audited source.

## Scope and method

The source was reviewed independently against stable `v0.0.8` at
`2e99b6a16e3c3bec7fdde88ae580c1d76cba8e56`. The review covered the complete
stable-to-0.0.9 change surface, including boot mapping configuration, GDT/TSS,
interrupt and syscall entry, page-table construction and permissions,
scheduler/process lifecycle, ELF parsing, user-copy, and test feature
selection. The CR3 and terminal handoffs were traced across Rust and assembly;
unsafe additions and panic paths were inspected separately. Passing tests were
treated as corroborating evidence, not as proof of the invariants.

The audit was limited to the x86_64, four-level, single-CPU model in this
release. It does not certify future SMP, VFS, storage, networking, or stable
userspace ABI behavior.

## Release scope

The implementation matches the 0.0.9 process/address-space/userspace
foundation. It does not implement VFS/initrd, `/bin/init`, file descriptors,
fork, general exec, dynamic linking, PIE/ASLR, signals, IPC/shared memory, a
user heap or `mmap`, demand paging, frame reclamation, storage, networking,
APIC redesign, SMP, or a stable ABI. `CHANGELOG.md` remains
`## [0.0.9] - Unreleased`; no 0.0.9 tag or release exists.

## Process identity and lifecycle

`ProcessId { slot: u8, generation: u64 }` and `Pid(u64)` are distinct from
each other and from `TaskId`. Slot lookup uses checked indexing and validates
generation and state. Generation zero is not issued; generation advances
only on reservation of a Vacant slot. Reap clears Zombie to Vacant without
advancing generation, so the old ID fails immediately; the next reservation
advances it. Checked generation overflow retires the slot. PID allocation is
checked, monotonic, and never wraps or reuses a number during a boot; exhaustion
returns `PidExhausted`.

The bounded eight-slot table implements Creating, Alive, Exiting, Zombie,
Vacant, and terminal Retired states. The reviewed transition path rejects
illegal transitions. Parent references retain generation-safe `ProcessId`s.
Metadata includes the bounded name, exit reason, tick accounting, mapped user
page count, task count, and bounded snapshots. CPU tick accounting saturates;
`mapped_user_pages` counts virtual user mappings and is not physical RSS.

Process policy remains outside scheduler policy. The scheduler records only
execution data such as task kind, task identity, saved context, stack slot, and
the process handle needed to select a root. Process names, parent policy, ELF,
PID, and exit metadata remain in the process subsystem.

## Publication, exit, and fault lifecycle

Creation keeps the process Creating while it constructs the AddressSpace, ELF
image, and user stack, then reserves a Prepared task that is not queued. Final
publication runs with interrupts masked and the scheduler lock held before
the process lock. It checks the task/process pairing and queue capacity,
publishes Alive, then inserts the task and marks it Ready. Queue insertion is
infallible after its capacity check while the same scheduler lock is held.
Thus the timer cannot observe a runnable task with a non-Alive process, or an
Alive process without its prepared primary task. Expected construction and
capacity errors are returned; failed physical allocations remain owned by the
monotonic frame allocator as documented.

`exit(status)` validates the current user task, records Exiting on both the
process and task, releases all locks, then enters the non-returning
`sti; hlt`-style terminal handoff. Interrupts are enabled only after mutable
syscall work is complete and locks are gone. The exiting task is not requeued.
The PIT saves the outgoing context and adopts a different context; a later
timer pass publishes Zombie. Reap is limited to Zombie. This places resource
reuse after the old kernel stack is inactive and avoids returning to the
faulting/user RIP.

The #PF, #GP, #UD, and #NM paths classify origin from saved CS privilege. CPL0
faults remain kernel-fatal; faults from a process task capture only trusted
scalar state (vector, error code when present, CR2 for #PF, and saved RIP),
then use the same non-returning Exiting handoff. No user strings or stack
contents are read for diagnostics. #PF captures CR2 before entering the
termination path. Zombie state and its fault metadata remain available until
reap.

## Boot mappings and virtual-address policy

Bootloader 0.11.17 is pinned in `Cargo.lock`. Its generated boot mappings use
the configured dynamic range. The implementation reserves the half-open
arena `0xffff800000000000..0xffff900000000000`, PML4 indices 256 through 287,
for those mappings. This is 32 PML4 entries, or 16 TiB of virtual capacity.
The bootloader range end is supplied as the final page sentinel before the
exclusive boundary because bootloader 0.11.17 rounds its configured end to a
PML4 index. pvKernel's explicit scratch/heap/task-stack mappings begin at
`0xffff900000000000` (index 288), outside the arena. The range is canonical,
page aligned, and below the canonical hole.

The generated kernel ELF inspected from the pinned toolchain is ELF64,
little-endian, x86_64 `ET_DYN` (type 3), with nine program headers and four
`PT_LOAD` segments. Its `DT_RELA` table contains 848 entries; independently
decoding the generated ELF showed every entry has relocation type 8
(`R_X86_64_RELATIVE`) and symbol index zero. The pinned bootloader loader
places this shared-object image in a free dynamic range, applies those
relative relocations, and enters at the relocated address. Runtime checks
then validate the actual relocated load segments, entry point, W^X
permissions, and arena membership rather than relying on configuration alone.

The local native UEFI QEMU boot observed these actual ranges:

| Mapping | Runtime range | PML4 indices |
| --- | --- | --- |
| Kernel load segments | `0xffff800000000000..0xffff80000006e5ba` across four PT_LOAD segments | 256 |
| Boot stack | `0xffff808000001000..0xffff808000015000` | 257 |
| Framebuffer | `0xffff810000000000..0xffff8100003e8000` | 258 |
| Physical-memory map | `0xffff818000000000..0xffff828000000000` | 259–260 |
| BootInfo and memory-region storage | `0xffff828000000000..0xffff828000000bb0` | 261 |
| Explicit kernel heap | begins at `0xffff900010000000` | 288 |
| Kernel task stacks | begin at `0xffff900020000000` | 288 |

The runtime audit verifies live boot mappings, the physical-map extent, the
kernel stack and current RSP, BootInfo/memory regions, framebuffer when
retained, kernel ELF segments, and the upper page-table tree. It fails closed
if a required mapping is not canonical, is outside the arena, is absent, or
has effective USER access. No lower-half kernel mapping is copied into a
process root. Process roots begin with an empty lower half and install only
validated user mappings. The low 64 KiB is forbidden; accepted user addresses
are `[0x10000, 0x0000800000000000)`. ELF and stack bands are disjoint from
kernel mappings, and range/page arithmetic is checked.

Every present entry in the audited kernel-half hierarchy is checked for USER
clear, including parent levels and huge-page leaves. Process roots clone the
audited kernel-half PML4 entries, and later activation verifies the clone
against the kernel root. User leaves are installed by the pinned
`x86_64` mapper, which propagates USER and WRITABLE through missing parent
tables; the process mapping API does not expose parent-entry mutation. User
mapping APIs enforce USER bounds, reject collisions, and reject writable plus
executable permissions.

## Address spaces and CR3 handoff

Each `AddressSpace` owns an inactive PML4 root and mapping/page-count
metadata. New root and intermediate table frames are zero-initialized before
use. Inactive roots are edited through the supervisor physical-memory map;
the kernel does not switch CR3 to construct them. Process-owned roots and
frames remain monotonic/kernel-lifetime allocations because general physical
frame reclamation is not implemented.

The scheduler constructs a SwitchPlan while holding scheduler state, then
releases scheduler/process locks. Timer IRQ0 receives exactly one PIC EOI
before architecture adoption. The production switch path validates the
selected opaque address-space handle under the kernel paging lock, releases
that lock, then activates the verified root while IF remains clear. The
runtime audit established identical upper-half mappings for code, current
kernel stack, scheduler/process state, GDT, IDT, TSS/IST, heap, task stacks,
and physical-map access. Therefore code and data touched after CR3 load remain
valid at the same virtual addresses. No process or paging lock crosses EOI,
CR3 load, RSP adoption, or `iretq`.

The kernel uses ordinary CR3 writes without PCID allocation or no-flush
switches. The bootloader installs the kernel root with empty CR3 flags, and
the kernel preserves the root's cache flags. User mappings are non-global;
ordinary CR3 writes flush non-global translations between roots. Shared
kernel mappings are identical in each process root.

## TSS, stacks, and context ABI

The TSS has a kernel-lifetime fallback RSP0 for bootstrap/kernel execution.
Before every `iretq` to CPL3, the selected process task's validated kernel
stack top is programmed as RSP0. User-to-user transitions select the target
task stack; user-to-kernel transitions restore the fallback before the old
user task stack can be reused. RSP0 setters require IF clear and validate the
top against initialized scheduler stack slots. Those stacks are supervisor,
read/write, non-executable, and guarded.

Each process has an eight-page (32 KiB) USER/RW/NX stack at
`0x00007ffffffff000` with one unmapped lower guard page. It does not grow
automatically. The user stack is only for CPL3 execution. CPL3-to-CPL0
interrupts and syscalls use the task's separate supervisor kernel stack.

The timer assembly pushes the 15 GPRs in reverse layout order and restores
them in the order represented by the 20-qword Rust `Context`, followed by
`iretq`. Compile-time size/offset assertions anchor RIP/CS/RFLAGS/RSP/SS.
The CPU creates a five-qword hardware frame on CPL3-to-CPL0 transitions; a
same-CPL0 interrupt creates only RIP/CS/RFLAGS. The scheduler reads the saved
CS and saved GPR prefix common to both, while synthetic first-run contexts
provide the full frame needed for a CPL3 return. The assembly preserves the
original register prefix before using R12 as a temporary, aligns the Rust
call stack, clears DF before calling Rust, and keeps IF clear through the
handoff. New user GPR values are zeroed and RFLAGS is initialized to the
sanitized `0x202` policy (reserved bit 1 and IF only); the separate initial
userspace entry path also uses the same sanitized flags.

## FP/SIMD execution-state policy

Tasks do not save or restore x87, MMX, SSE, or XSAVE-managed state. The pinned
`x86_64-unknown-none` target specifies `-mmx,-sse,-sse2,-sse3,-ssse3,-sse4.1,
-sse4.2,-avx,-avx2,+soft-float`; `kernel/build.rs` rejects enabled MMX/SSE/
AVX target features. Boot prerequisites clear EFER.SCE and SYSENTER targets,
clear and verify CR4.OSXSAVE, set and verify CR0.EM, and retain CR0.WP and
EFER.NXE. Thus no process/kernel preemptible path relies on unsaved FP/SIMD
state. Deterministic userspace fault fixtures exercise x87, legacy SIMD,
AVX-family denial, and #NM containment. This policy must be replaced with
explicit per-task state management before enabling those execution units.

## Syscalls, user-copy, and ELF

`int 0x80` remains the experimental interrupt-gate boundary. The gate is
CPL3-callable, clears IF, saves the scalar register frame, and returns with
`iretq`; ordinary handlers are non-preemptible. The process-aware additions
are `getpid` and terminal `exit`. No stable ABI is promised.

Central `copy_from_user`/`copy_to_user` validates checked end arithmetic,
canonicality, user bounds, mapped pages, effective USER access, and writable
destination pages; it preflights the full range and copies in page-sized
chunks through supervisor physical aliases, never by raw user-VA memcpy. The
current no-remap/no-TOCTOU argument depends on one CPU, one task per process,
immutable Alive mappings, no mmap/remap, no user page-table writes, IF clear
during ordinary syscalls, and no SMP. Threads, remapping, or SMP require a
stronger pin/lock/fault-safe copy design.

The loader accepts only ELF64 little-endian x86_64 static `ET_EXEC` images
with `PT_LOAD` and BSS. Checked decoding validates header/program-header
sizes and arithmetic, segment/file ranges, canonical/user/ELF bounds,
page rounding, alignment, page overlap, stack overlap, W+X, unsupported
dynamic/interpreter/TLS segments, and an executable mapped entry. It does not
cast hostile bytes to ELF structs. New user pages are zeroed before bytes are
copied; BSS and page padding remain zero before final permissions and before
publication. Final permissions are USER RX code, USER RW/NX writable data,
and USER RW/NX stack; x86 does not provide a read-disable permission bit.

## Unsafe code, panic paths, and hot paths

Every newly introduced unsafe block in the stable-to-audit diff was inspected.
The operations are limited to TSS cell access under the single-CPU/IF-clear
contract; validated CR3 writes; physical-map page-table and frame access;
bounded copies through validated physical aliases; the timer context ABI;
and deterministic fixtures. Each new block has a local SAFETY rationale
covering pointer validity/ownership, mapping lifetime, and the interrupt or
CR3 assumptions it needs. The context/assembly layout is compile-time checked
and exercised by timer, CPL3 preemption, and same-VA isolation QEMU scenarios.

Panics/expectations on invalid ELF, user-copy input, process/task capacity, and
faulting userspace are converted to typed errors or process termination.
Remaining assertions and `expect` calls guard boot ordering, static bounded
indices, scheduler state corruption, or deterministic test fixtures where
continuing would be unsafe. The PIT path performs bounded scheduler/table
work only: no heap/frame allocation, mapping, ELF parsing, filesystem access,
or ordinary switch logging. It accounts one tick to the task/process active
on IRQ entry using saturating process accounting, releases locks, sends one
EOI, and then performs the switch.

## Validation evidence

### Static and policy checks

| Check | Result |
| --- | --- |
| `git diff --check` | PASS |
| `cargo fmt --all -- --check` | PASS |
| `cargo build --locked` | PASS |
| `cargo metadata --locked --format-version 1 --no-deps` | PASS; workspace packages are 0.0.9 |
| `cargo test --workspace --locked` | PASS; runner reports 0 unit tests, kernel test harness is disabled |
| Kernel Clippy, default target, `qemu-test`, `process-smoke-test`, `user-copy-test`, each with `-D warnings` | PASS |
| `cargo audit` | PASS; one existing accepted warning, RUSTSEC-2025-0141 (`bincode` 1.3.3) on bootloader build-time path only |
| `cargo deny check` | PASS; existing unmatched-license and duplicate-dependency warnings only |
| `actionlint` | PASS |
| `python scripts/check_security_policy.py` | PASS |
| Python policy tests | 65 passed; one Windows-only hostile-filename fixture errors because Windows rejects `::` in the temporary filename; validator and repository-policy tests pass |

### Deterministic QEMU matrix

Every listed command was run on the audit source with the repository runner's
`--headless-test` mode. PASS requires runner host exit 0 and the listed
scenario marker. The runner maps only QEMU's expected `isa-debug-exit`
success status to host exit 0; serial output alone was not treated as a pass.

| Scenario | Feature selection | Required marker | Result |
| --- | --- | --- | --- |
| Timer boot | `qemu-test` | timer tick counter advanced successfully | PASS |
| Expected page fault | `qemu-test fault-tests` | expected page fault matched | PASS |
| Post-unmap fault | `qemu-test mem-fault-tests` | expected post-unmap page fault matched | PASS |
| Supervisor write protection | `qemu-test supervisor-write-protection-test` | expected supervisor write-protection fault matched | PASS |
| CPL3 syscall | `qemu-test userspace-syscall-test` | test_exit(success) from CPL3 payload | PASS |
| CPL3 privileged instruction | `qemu-test userspace-privilege-fault-test` | expected CPL3 privilege fault matched | PASS |
| CPL3 supervisor memory | `qemu-test userspace-supervisor-fault-test` | expected CPL3 supervisor-page fault matched | PASS |
| Heap allocation | `qemu-test heap-test` | heap tests completed successfully | PASS |
| Heap OOM | `qemu-test heap-oom-test` | expected allocation failure | PASS |
| Heap guard | `qemu-test heap-guard-fault-test` | expected heap guard-page fault matched | PASS |
| Scheduler smoke | `qemu-test scheduler-test` | scheduler smoke completed successfully | PASS |
| Kernel-task preemption | `scheduler-preemption-test` | preemption completed successfully | PASS |
| Task capacity/reuse | `qemu-test task-capacity-test` | stale TaskId rejected | PASS |
| Task stack guard | `task-stack-guard-fault-test` | task stack guard fault passed | PASS |
| Process smoke | `process-smoke-test` | userspace getpid passed | PASS |
| Process preemption | `process-preemption-test` | CPL3 timer preemption passed | PASS |
| Address-space isolation | `address-space-isolation-test` | same-VA isolation across timer switches passed | PASS |
| User stack guard | `user-stack-guard-fault-test` | fault-containment aggregate passed | PASS |
| User/kernel isolation | `user-kernel-isolation-test` | fault-containment aggregate passed | PASS |
| User fault containment | `user-fault-containment-test` | #PF/#GP/#UD/#NM containment aggregate passed | PASS |
| Process exit | `process-exit-test` | exit handoff reached Zombie safely | PASS |
| Process capacity | `process-capacity-test` | mixed task/process capacity and rollback passed | PASS |
| Process ID reuse | `process-id-reuse-test` | stale ProcessId rejected after reuse | PASS |
| ELF loader | `elf-loader-test` | static ELF loader and malformed-header tests passed | PASS |
| Malformed ELF | `elf-malformed-test` | static ELF loader and malformed-header tests passed | PASS |
| User-copy | `user-copy-test` | positive and negative cases passed | PASS |

### Ordinary graphical boot

The ordinary, non-test `cargo run --locked` path launched native Windows QEMU
with the graphical framebuffer. The runtime log showed the upper-half boot
mapping audit, heap initialization, process subsystem, framebuffer rendering,
and interrupt-enabled scheduler. Its embedded static ELF used the production
AddressSpace/loader/publication path, checked `getpid` against its assigned
PID, called non-returning `exit(0)`, reached deferred Zombie completion, and
was reaped. Timer progress continued and the kernel reached its normal idle
loop. QEMU was stopped from the host only after those normal-idle markers;
the interruption was the audit operator closing the intentionally persistent
normal boot, not a kernel failure.

### Exact-main GitHub evidence

All queried runs were completed push-event runs for exactly
`8370457139e1530467cc13ba7308b56eaf802eaf`:

| Workflow | Run ID | Conclusion |
| --- | ---: | --- |
| CI | `40117887470` | success; all listed build, Clippy, and QEMU jobs succeeded |
| Security | `40117887449` | success; cargo audit, cargo deny, actionlint, and CodeQL succeeded |
| Security triage | `40117887490` | success; policy validation/tests succeeded |

The Security workflow's `Dependency review (pull requests only)` job was
skipped because this was a push event on `main`; this is the documented
event-specific condition, not an unexplained required skip.

## Findings

| Severity | Finding | Release impact |
| --- | --- | --- |
| P3 | `docs/architecture/processes.md` still describes the implementation as on an “unmerged implementation branch”; a comment above `bootloader_config()` also says pvKernel keeps the default mapping layout despite constraining the dynamic range. These are stale status/comment text, not executable behavior. | Non-blocking documentation cleanup; no architecture invariant is obscured in the implementation or release-facing README/CHANGELOG. |
| INFO | One Python hostile-filename fixture cannot create its `::` filename on Windows; Linux CI policy tests pass and the standalone security-policy validator passes locally. | Environment-specific test-fixture limitation. |
| INFO | The existing accepted `RUSTSEC-2025-0141` warning is for `bincode 1.3.3` through the bootloader build-time dependency path; no new advisory appeared. | No runtime dependency path was added. |

No unresolved P0, P1, or P2 findings were identified. No release blocker remains.

## Verdict

**READY FOR RC1 REVIEW.** This verdict applies only to source commit
`8370457139e1530467cc13ba7308b56eaf802eaf` and the stated single-core x86_64
scope. It authorizes human review of this audit record; it does not create an
RC, tag, or release.
