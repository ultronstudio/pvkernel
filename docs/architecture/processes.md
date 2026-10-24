# pvKernel 0.0.9 process architecture

**Status:** authoritative 0.0.9 design and current implementation model on
the unmerged implementation branch; 0.0.9 is not released. The stable-
baseline sections describe pvKernel 0.0.8 at
`2e99b6a16e3c3bec7fdde88ae580c1d76cba8e56`. Phase A process identity,
Phase B boot-mapping audit/inactive-root construction, Phase C user-stack/
RSP0 support, Phase D checked static ELF loading, and the Phase E prepared
user-task/process-resource model are implemented on the 0.0.9 branch. Phase F
has an audited CR3 activation primitive, a Rust scheduler SwitchPlan, and a
QEMU runtime root-switch/RSP0 smoke. Phase G now schedules process-owned CPL3
timer frames and has a deterministic two-process preemption test; legacy
direct-from-bootstrap CPL3 boundary tests retain their non-scheduling path.
Phase H adds process-aware `getpid` and terminal `exit`, with deferred Zombie
publication after the outgoing kernel stack becomes inactive; an embedded
ELF smoke checks PID identity and non-returning exit. Phase I adds central
page-chunked `copy_from_user`/`copy_to_user` APIs with full-range preflight;
the dedicated QEMU test covers same/cross-page copies and hostile ranges.
Phase J routes CPL3 #PF/#GP/#UD from process Tasks into the terminal exit
handoff and defers Zombie publication until a later PIT runs on an adopted
stack; its QEMU scenario observes #PF/#GP/#UD, stack-guard and supervisor-
mapping faults, then reaps their Zombies. Phase K adds saturating PIT-based
process CPU accounting, a fixed-size `snapshot_all` API, real same-VA/
different-frame user-process switching, and mixed scheduler/process capacity
validation. Ordinary boots now run an embedded static userspace smoke ELF
through the same process and loader path.

## 1. Scope and goals

The milestone extends the proven single-core kernel-task scheduler to
isolated userspace processes. Its central rule is: **Process** owns an
address space and process-wide resources; **Task** is a schedulable execution
context. The first implementation may have one user task per process, while
leaving process identity independent of that initial one-to-one relationship.
The detailed decisions below remain the design contract for the implementation.

## 2. Current 0.0.8 baseline

The working tree was clean. `main`, `origin/main`, and `v0.0.8` resolve to
the commit above; `v0.0.8-rc.1` is historical. The workspace is a Rust 2024
nightly `no_std` kernel binary with the stable package version 0.0.8.

Authoritative code inspected includes `kernel/src/task/mod.rs`,
`kernel/src/arch/x86_64/{task,interrupts.rs,gdt.rs,syscall,memory,userspace}`,
`kernel/src/main.rs`, build/config files and the existing architecture,
testing, security and release documentation. Source code governs where an
older architecture document uses historical wording.

## 3. Current scheduler facts

`crate::task::SCHEDULER` is `Once<Mutex<Scheduler>>`. `Scheduler` owns an
initialized flag, `[TaskSlot; MAX_KERNEL_TASKS]`, a fixed circular
`ReadyQueue`, `current: Option<TaskId>`, and `bootstrap_rsp`.
`MAX_KERNEL_TASKS` is 8. Each slot has `generation: u64`, `TaskState`, saved
context RSP and a queued flag. `TaskId` is `{ slot: u8, generation: u64 }`;
lookups bounds-check the slot and check generation/state. States are
`Vacant`, `Ready`, `Running`, `Exiting`, `Exited`, `Retired`. Reuse increments
with `checked_add`; exhausted slots become permanently `Retired`. The queue
stores optional TaskIds in a bounded ring. The bootstrap/idle context is not
a task slot: the first IRQ saves it to `bootstrap_rsp`; `current == None`
identifies it.

Scheduler APIs wrap mutex access in `interrupts::without_interrupts`;
timer-gate entry already clears IF. In `timer_schedule`, task state and
selection updates happen inside this lock, which is released when the
closure returns. The PIC EOI is sent after lock release. Assembly adopts
the returned context pointer only after Rust returns. The synchronization
assumption is one CPU; interrupt masking is not SMP protection.

`task_exit` marks the running task `Exiting`, then waits on its own stack
with `sti; hlt`. A later PIT switch saves that context, marks it `Exited`,
and switches away. This two-phase rule prevents reuse of a live stack.

## 4. Current interrupt and context facts

PIT IRQ0 is PIC vector 32 (`IrqLine::Timer.vector()`). The IDT entry is
`pvkernel_timer_irq_entry` assembly. Push order is `RAX, RBX, RCX, RDX, RBP,
RSI, RDI, R8, R9, R10, R11, R12, R13, R14, R15`; memory at saved RSP is
therefore `R15..R8, RDI, RSI, RBP, RDX, RCX, RBX, RAX`, then the hardware
`RIP, CS, RFLAGS, RSP, SS` frame. This is the 20-qword `Context` (160 bytes).

The assembly aligns its temporary Rust call stack to 16 bytes, executes
`cld`, calls `pvkernel_timer_schedule`, assigns the returned pointer to RSP,
pops the selected GPRs, then executes `iretq`. The interrupt gate clears IF
until selected RFLAGS is restored. The current timer path records a tick;
for scheduled CPL0 contexts it releases scheduler lock, EOIs the PIC, and
then returns the chosen context to assembly. If saved `CS & 3 == 3`, it
records the tick, sends EOI and returns the same user context without
scheduling. Existing user tests prove timer delivery, not user preemption.
Hardware provides the five-word long-mode return frame used by this code;
on CPL3 entry it first switches to TSS.RSP0, while same-CPL entry uses the
current stack.

## 5. Current TSS / RSP0 facts

`arch::x86_64::gdt::init` creates GDT/TSS, puts the double-fault stack in
IST[0], and puts the top of one static 16 KiB `SyscallStack` in
`TSS.privilege_stack_table[0]` (RSP0). No code updates it after boot. CPL3
entry and `int 0x80` tests depend on it; syscall test code checks that its
saved frame is within this stack. Current CPL3 syscalls, exceptions and
timer entries therefore share that static stack. This cannot support
multiple preemptible userspace tasks because their live frames can overlap.

The static 0.0.8 syscall stack is a kernel-lifetime safe fallback RSP0 while
the current target is the bootstrap context or a kernel Task. Same-CPL0
interrupt entry uses the current RSP, not RSP0. Add a narrow checked
`set_current_rsp0(top)` operation later. Immediately before every `iretq`
that returns to CPL3, RSP0 must contain the validated kernel-stack top of
that exact userspace Task. Do not treat RSP0 as the current kernel task's
restore the safe fallback before the outgoing user stack can become reusable.

Implementation note: the TSS now has a single-core `UnsafeCell` wrapper for
RSP0 updates. `set_current_rsp0` rejects calls while IF is set and accepts
only an aligned top matching an initialized guarded task-stack slot.
`set_kernel_fallback_rsp0` restores the static syscall stack. QEMU verifies
both programmed values. The setter does not select tasks or change the
context-switch path.

## 6. Current paging, userspace and syscall facts

`memory::init` rejects `CR4.LA57`, verifies NX/write-protect prerequisites,
reads active CR3 and builds the kernel `OffsetPageTable` using the
bootloader's physical-memory offset. `KernelAddressSpace` and monotonic
`BootFrameAllocator` live behind one `Once<Mutex<_>>`. The runtime boot audit
checks the dynamic mapping inventory, kernel ELF `PT_LOAD` permissions, and
every present upper-half page-table entry for USER clear. `create_address_space`
allocates a zeroed inactive root, copies only audited PML4 entries 256..511,
and never copies lower-half entries. Inactive `OffsetPageTable` operations
allocate intermediate tables through the boot allocator and never switch
CR3. `AddressSpace` maps zeroed user frames only in the accepted lower-half
range and tracks mapped-user-page count; page-table leaves are the mapping
inventory. Frames remain monotonic and are not reclaimed. The bootloader's
physical map is used to reach inactive page-table frames.

QEMU's address-space smoke creates two roots, maps the same user VA to distinct
frames, writes distinct values, and verifies those values through the
supervisor physical map. It also checks that a kernel task-stack VA translates
to the same physical frame in both roots with USER clear, and that low and
upper-bound user pages remain unmapped. This validates root construction, not
CR3 switching or scheduled userspace execution.

The active root starts with bootloader mappings. Kernel image, boot stack,
boot info, framebuffer and physical-memory mapping use dynamic placement;
the kernel does not keep a numeric manifest of them. Kernel-created ranges
are documented in section 7. User tests currently map code/stack inside
`USERSPACE_RANGE_BASE=0x0000_6000_0000_0000` in that same active root; this
does not prove address-space isolation. W^X mapping permissions include
supervisor RW/NX, supervisor RO/NX, user RW/NX and user RO/X. USER must be
clear through every parent as well as the leaf. No current mapping sets
GLOBAL. Mapping operations use 4 KiB pages; they reject parent huge pages.

The provisional `int 0x80` gate is DPL3. Its assembly entry saves all 15
GPRs on RSP0, calls the scalar dispatcher, writes its return to saved RAX,
restores registers and `iretq`s. As an interrupt gate, it clears IF; PIT
cannot preempt a syscall body. Keep this non-preemptible syscall policy.

The 0.0.9 context frame does not save x87, MMX, SSE, or AVX state. Before
userspace runs, the kernel therefore sets CR0.EM, clears CR4.OSXSAVE and
EFER.SCE, and zeros IA32_SYSENTER_CS/ESP/EIP. This contains x87/legacy-SIMD
and AVX use as process faults and prevents inherited firmware SYSCALL or
SYSENTER targets from bypassing the `int 0x80` boundary. Saving floating-point
state and enabling those instruction families requires a later explicit
per-task state design.

## 7. Current virtual-address map

Four-level canonical ranges are lower `0x0000_0000_0000_0000` through
`0x0000_7fff_ffff_ffff`, upper `0xffff_8000_0000_0000` through
`0xffff_ffff_ffff_ffff`. Exact dynamic bootloader ranges cannot be inferred
as constants from current source.

| Region | Current address/range | Notes |
|---|---|---|
| Kernel image | Bootloader dynamic arena; current QEMU base `0xffff800000000000` (PML4 256) | ELF `PT_LOAD` pages are audited from the loaded kernel ELF; `BootInfo.kernel_len` is the ELF backing length, not a contiguous virtual extent |
| Bootloader kernel stack | Current QEMU `0xffff808000001000..0xffff808000015000` (PML4 257) | Guard page immediately below; runtime range is audited |
| Framebuffer | Current QEMU `0xffff810000000000..0xffff8100003e8000` (PML4 258) | Retained by `kernel_main`; exact byte range is registered and audited |
| Physical-memory map | Current QEMU `0xffff818000000000..0xffff828000000000` (PML4 259–260) | 1 TiB physical-address coverage in this QEMU memory map, including address holes/MMIO; not 1 TiB of RAM |
| BootInfo + memory-region slice | Current QEMU `0xffff828000000000..0xffff828000000bb0` (PML4 261) | The allocator retains the memory-region slice; both ranges are audited |
| Scratch | `0xffff_9000_0000_0000` | Test-only; checked unmapped before use |
| Kernel heap | `0xffff_9000_1000_0000..0xffff_9000_1010_0000` | 1 MiB supervisor RW/NX; guard pages at `...0ffff000` and `...10100000` |
| Kernel task stacks | `0xffff_9000_2000_0000..0xffff_9000_2003_0000` | 8 x 16 KiB RW/NX supervisor stacks; one unmapped page on each side; 24 KiB stride |
| User test code | `0x0000_6000_0000_0000` | One test-only RX+USER page in the current root |
| User test guard | `0x0000_6000_0000_1000` | One deliberately unmapped page between code and stack |
| User test stack | `0x0000_6000_0000_2000..0x0000_6000_0000_3000` | One test-only RW/NX+USER page in the current root |
| User supervisor-fault probe | `0xffff_9000_0000_2000` | Test-only supervisor page near scratch base |
| Other reservations | No global reservation registry | Collision checks are local to mapping callers |

The explicit scratch, heap and task-stack ranges are all in PML4 index 288;
the current test user pages are in lower-half PML4 index 192. QEMU confirmed
that the bootloader's default dynamic mapping previously placed the kernel at
`0x0000_0100_0000_0000` (lower-half PML4 2). The implementation now configures
the exact bootloader 0.11.17 dynamic allocator to use a dedicated upper-half
arena, and runtime inspection confirms the required mappings in PML4 indices
256–261. Bootloader identity mappings in lower-half PML4 entries remain in the
kernel's initial root, but are not required after the kernel replaces the
bootloader GDT and begins using the audited upper-half code, stacks, and
physical-memory map; process roots do not copy them.

## 8. Proposed user virtual layout

Use only the lower canonical half (`USER_MIN=0x0000_0000_0001_0000`,
exclusive `USER_MAX=0x0000_8000_0000_0000`). Keep addresses 0 through 64 KiB
unmapped, including null and low aliases. USER_MAX is an exclusive bound,
not dereferenceable. All arithmetic and rounding is checked. This layout is
compatible with 48-bit canonical addressing and `LA57=0`. All retained
bootloader mappings are now constrained to the upper half, so no lower-half
PML4 slot is reserved for kernel mappings.

| Region | Proposed layout |
|---|---|
| Forbidden low | `0..0x10000`, unmapped |
| ELF ET_EXEC | `0x0000_0000_0040_0000..0x0000_4000_0000_0000` |
| Future heap | `0x0000_4000_0000_0000..0x0000_6000_0000_0000`, reserved only |
| Future mmap | `0x0000_6000_0000_0000..0x0000_7000_0000_0000`, reserved only |
| Future growth area | `0x0000_7000_0000_0000..USER_MAX`, reserved |
| User stack | `USER_STACK_TOP=0x0000_7fff_ffff_f000`, exclusive; 8 pages / 32 KiB below it |
| User stack guard | One unmapped page below the mapped stack |

The user stack's low mapped address is TOP - 8*4096; guard is the prior
page. ELF segments may not overlap stack or guard. Historical test base
`0x6000_0000_0000` is isolated per process and lies in the reserved future
mmap band. Root creation still checks actual boot mappings for collision.

## 9. Process vs Task

Keep `ProcessId` separate from existing `TaskId`. Process owns AddressSpace,
PID, parent, name, lifecycle, accounting and exit metadata. Task owns saved
context, kind, kernel stack and runnable state. Initially `TaskKind::User(ProcessId)`
is one task per process. Later multiple tasks may hold the same ProcessId.

Add only execution fields to a scheduler slot: `TaskKind`, kernel-stack top,
and opaque address-space/process handle (or ProcessId resolved into an
architecture `SwitchPlan`). Keep ELF, PID, PPID, names, exit code, filesystem
and future FDs out of scheduler policy. Do not spread naked CR3 values into
generic scheduler code.

## 10. ProcessId

Use `ProcessId { slot: u8, generation: u64 }`, matching current TaskId widths
and an 8-slot process table. Generation 0 is never issued. Advance the
generation with `checked_add` when reserving a new identity from a reusable
Vacant slot; the first reservation advances zero to one. If increment
overflows, mark that slot Retired and try another. Lookup checks bounds,
generation and state before access; invalid/stale IDs return typed errors
or `None`; Vacant and Retired slots never identify a live process. Reaping
Zombie to Vacant does not increment generation: Vacant already makes the old
ID invalid. A later reservation increments it before issuing the next ID.
Failed Creating attempts may consume a generation. This is an internal
handle, not a pointer, PID or unchecked array index.

## 11. PID

Use `Pid(u64)` for userspace/log identity. Allocate monotonically per boot
with checked increment and never reuse, even after reap. On exhaustion,
return `PidExhausted`; never wrap. PID is not an authorization handle;
internal parent and object references use ProcessId.

## 12. Process table

Use a bounded static table with `MAX_PROCESSES=8`. Eight records of a few
hundred bytes cost only a few KiB; correctness must not require heap
allocation. A slot records generation, lifecycle, PID, optional ProcessId
parent, AddressSpace ownership, primary TaskId, bounded name, create/exit
ticks, CPU ticks, mapped user pages, task count and exit metadata. `Creating`
is private and not schedulable. Capacity exhaustion returns
`ProcessTableFull`, not panic.

Do not hold process and scheduler locks over context switch. Define one lock
order for the final transaction (scheduler then process table if scheduler
selection resolves ProcessId while holding scheduler state). Prepare the
primary task first: reserve its slot, stack and context in a non-runnable
Reserved/Prepared state, but do not queue it or make it runnable. Keep the
process Creating until its address space, ELF and user stack are complete.
Then, with interrupts masked and both locks
held, validate the pairing and queue capacity, enqueue the prepared task,
mark the process Alive, and only then release the locks/restore IF. On this
single CPU, timer scheduling cannot observe the intermediate state; the
scheduler lock also prevents selection before Alive is published. The
enqueue operation must be fallible without partial insertion. If task-slot
reservation or queue-capacity validation fails, leave the process Creating
and roll back the prepared task before returning an error. There must be no
path where a runnable Task references a non-Alive Process, or an externally
observable Alive Process lacks a task that can be made runnable. Process
publication itself is an infallible state write after validation; if that
assumption changes, add rollback while locks and interrupt masking still
prevent observation.

Implementation note: each process slot now owns an optional `AddressSpace`
and primary `TaskId` while Creating. Scheduler slots distinguish Kernel and
User(ProcessId) tasks and include a non-queued `Prepared` state. Synthetic
user contexts are initialized with user selectors, checked entry/RSP bounds,
safe RFLAGS, and zero GPRs. QEMU verifies a Prepared task remains non-runnable
while its process is Creating, and that cancellation plus Creating rollback
leaves no schedulable task. Final publication will be exercised together
with CR3-aware scheduling in later phases.

## 13. Lifecycle, Zombie and reap

Legal lifecycle: `Vacant -> Creating -> Alive -> Exiting -> Zombie -> reap
-> Vacant`, plus `Retired` when a new reservation cannot advance generation.
Generation advances at reservation, not at reap. Failed creation returns
Creating to Vacant; its consumed generation is not rolled back. Alive
means a valid AddressSpace and user task exist. Exiting cannot be selected
for ordinary new work, though it may still execute on its kernel stack.
Zombie has no executing task and retains exit data. Only Zombie may be
reaped. Reaping changes Zombie to Vacant, which immediately invalidates the
old ProcessId; the next reservation advances generation.
Creating, Exiting or Zombie must never be enqueued; Alive requires all
address-space and task invariants.

Zombie retains PID, parent, exit status/reason, CPU accounting and fault
metadata for future wait/reap, init, service supervision, Crash Viewer and
System Monitor. No waitpid is implemented now.

## 14. Parent and exit reason

Store `parent: Option<ProcessId>`, not only PID. Root/test process may have
None. A stale parent handle remains safely detectable; future PID 1/service
manager may adopt orphans by updating it. No init semantics are implemented.

Start `ProcessExitReason` with `Exited { status: i32 }` and
`Fault { vector: u8, error_code: Option<u64>, fault_address: Option<u64>,
rip: u64 }`. Leave enum room for Killed, Signal, SecurityViolation and
OutOfMemory. Capture only trusted scalar hardware state; do not inspect user
memory to report a fault.

## 15. Fault containment

Exception classification uses saved CS RPL as the primary source. CPL0
faults retain existing kernel-fatal behavior. CPL3 #UD/#GP/#PF from a
process-owned Running Task capture only vector, optional hardware error code,
CR2 for #PF, and saved RIP. #PF USER_MODE remains consistency information,
not the sole classifier. The handler calls the common non-returning terminal
handoff while IF is clear; it marks Process and Task Exiting and releases all
locks before the `sti; hlt`-equivalent wait permits PIT delivery. Timer saves
the CPL0 handoff context on that process's kernel stack, selects another
context, and never restores the faulting user frame. Zombie is finalized on a
later PIT after a different stack has been adopted. The QEMU scenario runs
separate #PF/#GP/#UD processes, checks their scalar metadata and Zombie
states, then reaps them while the kernel continues.

## 16. AddressSpace and frame ownership

Create an owned `AddressSpace` abstraction around a root PML4 frame and
page-table/mapping ownership metadata. It provides checked map/unmap/translate
and user-range inspection; only x86 switch code deals in CR3. Candidate
metadata includes root, intermediate-table frames, owned user frames/mappings
and mapped-page count. Choose storage consistent with actual heap/metadata
limits.

The current BootFrameAllocator is monotonic and has no reclamation. AddressSpace
tracks which frames/mappings belong to it; page mappings may later be torn
down and process metadata reaped, but allocated backing frames remain
kernel-lifetime owned in 0.0.9. Preserve ownership records for future general
reclamation; do not claim a buddy allocator or returned physical memory.

## 17. Kernel-global mappings and CR3 switching

Each process root must map kernel text/rodata/data, heap, scheduler/process
state, active kernel task stacks, timer/syscall/exception entry code, GDT,
IDT, TSS, double-fault IST, physical-memory/table-walk support, and any
bootloader mapping still used (e.g. framebuffer), at identical virtual
addresses. PIC/PIT are currently port-I/O devices and need no virtual mapping;
future MMIO must be inventoried. All kernel leaves and parent entries must
have USER clear.

The lower canonical half (PML4 0..255) is exclusively userspace; PML4
256..511 is kernel-only. Bootloader 0.11.17's `dynamic_range_start` and
`dynamic_range_end` are applied by its `UsedLevel4Entries` allocator at PML4
granularity to every `Mapping::Dynamic` allocation, including an ET_DYN
kernel image. pvKernel constrains that arena to
`0xffff800000000000..0xffff900000000000` (PML4 256..287), immediately below
its explicit index-288 mappings. The API treats the configured end as an
address whose containing PML4 entry remains eligible; pvKernel passes the
last page in index 287 (`0xffff8ffffffff000`) so index 288 onward is excluded.
This arena has 32 PML4 entries / 16 TiB of virtual capacity. The QEMU boot
uses six entries (kernel image, stack, framebuffer, two for the 1 TiB physical
map, and BootInfo/memory map), leaving 26 entries / 13 TiB before index 288.
Bootloader 0.11.17 panics with `no usable level 4 entries found` if a dynamic
mapping cannot obtain enough contiguous entries; arena exhaustion is a boot
failure, not a recoverable kernel allocation error.

The default dynamic placement was observed to put the image in lower PML4 2.
That placement is not retained as a 0.0.9 compatibility model: bootloader
mapping policy moves all retained dynamic mappings into the dedicated upper
arena. The kernel verifies actual BootInfo ranges and all loaded kernel
`PT_LOAD` pages, checks the physical-map extent, confirms each required VA is
canonical and upper-half, and recursively audits every present upper-half
page-table entry (including huge pages) for USER clear. Process roots copy
only audited upper-half PML4 entries. Lower-half bootloader identity mappings
are not required by live kernel state and are deliberately omitted. Any
required lower-half mapping or USER bit in the kernel half fails closed before
a process root can be activated.

New roots remain reachable through the shared physical mapping. Inactive-root
construction is implemented and covered by the QEMU smoke test. CR3 switching
correctness requires
the executing kernel stack, switch assembly, scheduler state, GDT, IDT and
TSS to remain mapped at identical VAs in the target. The mapping audit and
clone verification establish that invariant; failure to establish it blocks
process scheduling.

Implementation note: `KernelAddressSpace::activate_address_space` checks IF=0,
compares every upper PML4 entry with the kernel root (ignoring only the
hardware-managed Accessed bit), and only then loads the process CR3. The full
recursive boot/live mapping audit runs at boot and during root construction,
not on the PIT hot path; scheduler-time verification is bounded to the 512
top-level entries. The scheduler constructs a `SwitchPlan` under scheduler
state protection, releases locks, acknowledges PIT, then applies the root and
selected-task RSP0 before returning the context pointer to assembly. The QEMU
runtime smoke alternates two roots whose
`USER_MIN` pages contain `0xAA` and `0xBB`, reads through the active VA each
time, then restores the kernel root. This proves root-load continuity and
same-VA translation isolation; it does not yet prove scheduler handoff.

## 18. User and kernel stacks

Give every process a dedicated 32 KiB user stack (8 pages), USER, writable,
NX; one downward guard page remains unmapped. Initial RSP is the exclusive
page-aligned USER_STACK_TOP. Use checked stack arithmetic and zero frames
before USER publication. Do not overlap ELF segments, stack or guard.

Implementation note: `UserStack::map` applies this fixed layout to an
inactive AddressSpace, rejects collisions, initializes every page through
the zeroed-user-page mapping path, and rolls back installed leaves on error.
The QEMU stack test verifies all eight effective USER/RW/NX mappings and the
unmapped lower guard. It does not yet enter CPL3 using this stack.

User stack is for ordinary CPL3 execution. Kernel stack is for CPL3→CPL0
timer/syscall/exception entry and handler execution. A process still needs a
scheduler-owned supervisor-only RW/NX kernel stack with guards; never use
the user stack for interrupt entry. Future threads each need their own
kernel stack.

## 19. TSS.RSP0 and CPL3 preemption

Immediately before every `iretq` that returns to CPL3, TSS.RSP0 MUST contain
the validated kernel-stack top belonging to that exact selected userspace
Task. Selecting user task B therefore chooses B's AddressSpace and kernel
stack, loads B's root if needed, sets RSP0 to B's top, then restores B's
context and returns to CPL3. For bootstrap or kernel-task execution, use the
existing static 0.0.8 syscall stack as a kernel-lifetime fallback; RSP0 does
not participate in same-CPL0 interrupt entry and need not identify the
current kernel Task. On user A → kernel/bootstrap, restore that fallback so
RSP0 does not point at A's stack once it can be reused. On user A → user B,
set B's top before B's iretq. Do all CR3/RSP0/context handoff with IF clear.

Future timer order: hardware enters on current CPL stack or task RSP0;
assembly saves all GPRs and full frame; Rust records tick/accounting and
captures current context; under scheduler lock update state/select a small
SwitchPlan; release lock; send PIC EOI; switch CR3 while current kernel stack
and code are mapped in destination; set RSP0 to the selected user Task top,
or the safe fallback for a kernel/bootstrap target; return selected context
pointer; assembly adopts RSP, restores registers and `iretq`. No process or
scheduler lock crosses EOI, CR3 switch, RSP adoption or iretq. IF remains
clear, preventing maskable same-core reentrancy during transition. NMI and
double fault require shared mappings/IST treatment. A terminal exit/fault
path is the deliberate exception to ordinary syscall non-preemption: after
all mutable syscall/exception work is complete, state is Exiting and locks
are released, it explicitly permits the PIT handoff as described below.

| Transition | CR3 | RSP0 |
|---|---|---|
| kernel task A → kernel task B | unchanged kernel root | safe static fallback remains; RSP0 is not B's current stack |
| kernel → user A | kernel → A | set A top before iretq |
| user A → kernel/bootstrap | A → kernel | restore safe static fallback before A's stack can be reused |
| user A → user A | unchanged | restore A top |
| user A → user B | A → B | set B top |

Implementation note: `timer_schedule` now classifies the saved CS, requires it
to match the current TaskKind, and schedules a CPL3 frame when that frame
belongs to a process-owned Running user Task. The existing timer Context is
saved on that task's RSP0 kernel stack; the selected process root and RSP0 are
applied before assembly restores the selected frame. Direct CPL3 boundary
tests that run from bootstrap with no current Task retain the legacy return-
same-context behavior. The feature-gated `process-preemption-test` starts two
non-yielding CPL3 loops and succeeds only after A and B each set an in-page
flag after reading the peer's counter under PIT-driven task switching. It uses
a PIT-tick watchdog and no host wall-clock criterion. The shared page mapping
exists only in this QEMU test harness. Ordinary int 0x80 handlers remain non-preemptible;
pending PIT delivery occurs after `iretq` restores IF. Do not switch processes
mid-syscall.

## 20. Context representation

Prefer the existing 15-GPR + five-word frame, already used by synthetic
kernel task startup and timer switching. Prove in assembly/tests that CPL0
and CPL3 timer frames share its 20-qword layout. A synthetic user frame uses
GDT user CS/SS, validated executable RIP, mapped user RSP, and fixed RFLAGS
`0x202` (reserved bit 1 and IF set; DF, IOPL, TF, NT, VM clear). Clear user
registers rather than leak kernel values. Exceptions with hardware error
codes are not timer Contexts; normalize them explicitly before any common
handoff.

## 21. Syscall policy and initial surface

Keep provisional `int 0x80`, with no SYSCALL/SYSRET migration. Interrupt-gate
entry clears IF, so the maskable PIT cannot preempt ordinary syscall Rust
code. `getpid()` returns the PID only when the current scheduler Task is a
Running user Task; bootstrap CPL3 tests get `InvalidContext`. `exit(status)`
marks the Process and Task Exiting, releases locks, then uses the explicit
STI;HLT handoff. The next PIT saves the outgoing frame and switches away;
Zombie is published only on a later interrupt after the old kernel stack is
inactive. The embedded smoke ELF verifies getpid and that exit does not
return. Process creation stays an internal kernel/test operation. Do not add
fork, exec/spawn, open/read/write, mmap, brk, signals or filesystem calls.

## 22. User-copy design

The implementation centralizes `copy_from_user`/`copy_to_user` in
`process::user_copy` and resolves only the current Running user Task's
generation-safe ProcessId. It validates low/null policy, 48-bit canonicality,
the full range in `[USER_MIN, USER_MAX)`, checked `start + len`, every touched
page's effective USER permission, and writable destination for copy-to-user.
Unaligned and cross-page ranges are copied in page-bounded chunks through the
supervisor physical map. The entire range is preflighted before any copy, so
an unmapped later page cannot partially update a kernel output buffer or user
destination. Zero-length copies succeed without dereferencing or validating
the pointer.

Do not validate then raw-memcpy user VA. Copy in page-sized chunks through
the kernel physical-memory mapping after translation. This is safe only
under 0.0.9 constraints: mappings are immutable while Alive, no user remap,
mmap, demand paging, concurrent user threads or SMP; syscall IF is clear,
single-core, and mapping edits cannot run during copy. Hold the address-space
ownership guard and do not yield. Future remapping, threads or SMP invalidate
this argument and require pinning/locking or fault-safe copy primitives.

## 23. ELF scope and hostile-input validation

Accept only ELF64, little-endian, x86_64 `ET_EXEC`, static executable,
`PT_LOAD`, BSS. Reject ELF32, other machine/endian/type, PT_INTERP, dynamic
linker/shared libraries, PIE unless explicitly reconsidered, relocations,
TLS and demand paging. Parse checked fields from bytes (or narrowly audit a
parser); do not default to unsafe struct casts over hostile input.

Validate magic/class/endian/machine/type, ELF/header and program-header
sizes, program-header offset/count/table overflow, `p_offset+p_filesz`,
`p_vaddr+p_memsz`, `filesz <= memsz`, input file bounds, canonical user
addresses and complete segment range inside user range, page-rounding
overflow, segment and page-rounded overlap, stack/guard collision, supported
alignment/flags, and W+X rejection. Entry must be canonical, user-range,
mapped, and inside an executable segment. Malformed data returns typed
loader errors, never panic.

Implementation note: the byte parser uses checked field decoding and
validates the complete image before allocating. To keep hostile-input work
bounded, this implementation accepts at most 16 load segments and 4096
mapped image pages (16 MiB). It rejects page-rounded overlap and stack/guard
overlap. QEMU loads a two-segment test image, checks RX/RW/NX permissions,
zero BSS and page padding, and rejects malformed identity/type/range/overlap/
W+X/entry cases. This test validates loading and mapping only; it does not
execute the ELF entry point.

## 24. W^X, BSS and user-page publication

Map `PF_X` executable, `!PF_X` as NX, `PF_W` writable; reject PF_W+PF_X.
x86 has no separate ordinary-page read permission bit: present user pages
are readable, with write and execute separately constrained. Reject
page-rounded segment overlaps for the initial narrow loader rather than
merging conflicting permissions.

Zero newly allocated frames before user visibility. Copy `p_filesz`, zero
`p_memsz-p_filesz` and page padding, install final permissions and USER bits
only after initialization, then verify. Keep process Creating and task
unqueued throughout. No stale RAM reaches userspace.

## 25. Process creation transaction

Reserve process slot/generation, checked PID, parent/name/create tick; create
AddressSpace and audited kernel mappings; parse/validate ELF; allocate/map/
zero segments and stack. Prepare/reserve a scheduler task slot, kernel stack
and synthetic context, but keep it unqueued and unrunnable. Verify selectors,
RIP/RSP/permissions and the Process/Task pairing. With interrupts masked,
atomically enqueue the prepared task and publish Process Alive while holding
the locks in the documented order. Queue capacity and task-slot availability
must be checked before publication; queue insertion must not partially
succeed. If reservation, validation or capacity fails, cancel the prepared
task, return process metadata to Vacant (or Retired if a future reservation
cannot advance its generation), and leave no runnable task. Allocated
frames/tables cannot be returned by the monotonic allocator and remain
kernel-lifetime owned; record retained allocations. Consume PIDs and any
reserved ProcessId generation on failed creation rather than reuse them.
The invariant is bidirectional: no runnable Task refers to a non-Alive
Process, and no Process is externally observable Alive without a prepared
primary Task that was successfully enqueued.

## 26. Exit, fault termination, Zombie and reap

Ordinary syscall handlers remain non-preemptible: the `int 0x80` gate clears
IF, mutable syscall work completes, and the handler returns normally through
`iretq`. Terminal `exit(status)` is different. It marks Process Alive→Exiting
and Task Running→Exiting under the required locks, releases every lock, then
enters a non-returning exit handoff. Only after that state is safe does the
single-core handoff explicitly permit maskable timer progress, using a loop
equivalent to the stable kernel-task `sti; hlt` wait. The PIT interrupt saves
the exit context and schedules another task; the exit syscall never returns
to userspace. This deliberate scheduler transfer does not make ordinary
syscall code preemptible.

User faults follow the same terminal sequence: capture trusted scalar
`Fault(...)` metadata, mark Process and Task Exiting, release locks, then
permit the timer handoff. No path returns to the faulting RIP. The scheduler
records pending completion when it saves the exiting context, but Process
becomes Zombie only when a subsequent timer entry runs on the newly adopted
context's stack; this deferred completion proves the old kernel stack is
inactive. Do not release its root or stack before then. An internal reaper
accepts only Zombie with no executing task, changes
it to Vacant (making the old ProcessId invalid immediately), and removes
metadata. Generation advances only on a later reservation; PID is never
recycled. Public waitpid is not required now.

## 27. Accounting and process names

Internal read-only snapshot: PID, ProcessId, parent PID if parent handle is
live, state, name, created/exit tick, CPU ticks, mapped user pages, task
count and exit reason. Future System Monitor, Service Manager, Process
Inspector and Crash Viewer can build on it; no syscall exposure now.

Use fixed 32-byte ASCII/UTF-8 storage plus length, with explicit reject or
truncate policy; no heap string. Name is descriptive, not authority.

At each PIT scheduling tick, charge one tick to the task running at the
boundary; user task ticks charge its process, including syscall kernel time
because syscall IF is clear and task remains current. Kernel tasks may have
per-task or kernel aggregate accounting. Use checked/saturating behavior
with documented overflow, never wrap silently. `mapped_user_pages` counts
present user 4 KiB leaf mappings owned by process; it is not RAM, RSS or
working set. Percentage calculation belongs in userspace.

## 28. Capacity review

Current eight task stacks use 128 KiB mapped physical stack backing and
192 KiB virtual span (eight 24 KiB slots, each 16 KiB stack plus two guards).
Increasing to 16 would add 128 KiB backing and 192 KiB virtual span. Table
metadata is small. Retain 8 shared scheduler slots and add at most 8 process
slots initially; process and kernel tasks compete for the same slots. The
bootstrap context is outside the table. Test mixed capacity. Any future
capacity change updates arrays, stack arithmetic, process bounds and tests
together.

## 29. Module layout and ABI

Likely split, consistent with current `task`/`arch` ownership:

```text
kernel/src/process/{mod.rs,id.rs,table.rs,state.rs,address_space.rs,
                    elf.rs,user_copy.rs,accounting.rs}
kernel/src/arch/x86_64/process/{mod.rs,cr3.rs,enter.rs,user_stack.rs}
kernel/src/arch/x86_64/memory/{address_space.rs,paging.rs,...}
kernel/src/task/{scheduler policy and context integration}
```

Keep lifecycle generic, scheduler policy separate, and CR3/TSS/privilege
mechanics architecture-specific. Exact files should follow existing module
ownership, not be mechanically created.

Consider a tiny future no_std `pvkernel-abi` with ABI version, syscall
numbers, scalar types and error codes only. No Process, AddressSpace, TCB,
kernel pointer or page-table internals. 0.0.9 ABI is experimental; first
meaningful compatibility checkpoint is 0.1.0. No crate created now.

## 30. QEMU validation plan

Use deterministic serial markers and PIT-tick watchdogs, never host wall
time. Intentional user faults pass only when process termination and
continued kernel activity are observed.

| Scenario | Setup / purpose | Required success; failure |
|---|---|---|
| `process-smoke-test` | Embedded ELF enters CPL3, checks getpid, calls exit, then contains UD2 sentinel | Zombie retains status 0, sentinel never executes, kernel reaps, QEMU passes; PIT watchdog fails otherwise |
| `process-preemption-test` | Two non-yielding CPL3 loops increment separate counters on a test-only shared page and set peer-observed flags | PIT handler passes only after both peer-observed flags are set; tick watchdog fails otherwise |
| `address-space-isolation-test` | A/B map same VA to different frames with AA/BB and alternate repeatedly | each always sees own value over A→B→A; tests CR3/TLB/scheduler |
| `user-stack-guard-fault-test` | User writes below mapped stack | process fault recorded, kernel continues |
| `user-kernel-isolation-test` | CPL3 reads/writes supervisor mapping | controlled fault/denial; kernel data unchanged |
| `user-fault-containment-test` | Separate #PF/#GP/#UD processes | metadata and Zombie observed, other work continues, no panic/return |
| `process-exit-test` | Known exit status then sentinel user instruction | sentinel never runs; Zombie retains status |
| `process-capacity-test` | Fill shared slots and request one more | typed error; no panic/partial publication |
| `process-id-reuse-test` | Reap and reuse slot | new generation/PID; old ProcessId rejected |
| `elf-loader-test` | Valid static ELF with data and BSS | enters; zero BSS/padding; expected exit |
| `elf-malformed-test` | Bad magic, ELF32, endian, machine/type, PT_INTERP, filesz>memsz, offset/address overflow, out-of-range/overlap, W+X, invalid/non-executable/noncanonical entry | deterministic loader error, no panic/partial process |
| `user-copy-test` | Valid same/cross-page read/write; null, low, kernel, noncanonical, overflow, second page missing, supervisor page, read-only output | valid pass; invalid returns controlled error without crash |

Before VFS/initrd, release smoke uses embedded static ELF bytes. Replace its
provider with initrd/VFS in 0.0.10. No filesystem support is implied.

## 31. Security threat review

| Threat | Required control |
|---|---|
| stale ProcessId, generation wrap, slot reuse, stale parent | bounds + generation + checked increment + retired slot; parent is ProcessId |
| PID confusion | monotonic PID for display; never internal authority |
| kernel mapping accidentally USER | check effective USER through every page-table level in every root |
| A sees B memory, wrong CR3 or missing kernel map | same-VA/different-frame runtime test, switch-plan checks and mapping audit |
| page-table frame aliasing | AddressSpace ownership/uniqueness checks |
| user W+X or stale BSS/page data | W^X validation and zero before USER publication |
| wrong/stale RSP0, cross-process stack, user-accessible kernel stack | selected task top, IF-clear update, supervisor RW/NX mapping and guards |
| user-copy overflow, noncanonical, cross-page hole, kernel pointer, RO destination | checked range and every-page effective permissions; chunk copies via kernel map |
| mapping race during copy | immutable Alive mappings/no threads/no SMP now; pin/lock/fault-safe copy later |
| malformed ELF and entry abuse | checked decode/arithmetic, strict allowlist, validate before publish |
| lock held across switch; Zombie scheduled; wrong task/root | build switch plan while locked, release before EOI/CR3/RSP, assert state |
| exit while executing | two-phase stack handoff; no early stack/root reclamation |
| CPL3 fault treated fatal or CPL0 contained | classify saved CS; CPL0 fatal, user fault scalar capture and nonreturning handoff |
| dead RIP resumed or user memory read for fault report | scheduler handoff never returns; report trusted hardware scalars only |

**Implementation gate:** dynamic boot mappings have no repository-owned
manifest. Section 17's runtime audit and root-clone verification are
mandatory; do not schedule userspace if they cannot prove shared mappings.

## 32. Explicit non-goals

No VFS, initrd, filesystem, `/bin/init`, FDs, fork, general exec/spawn syscall,
dynamic linker, shared libraries, PT_INTERP, PIE/ASLR unless reconsidered,
userspace multithreading API, signals, pipes, PTY, shared memory, general
IPC, user heap, brk, mmap, demand paging, COW, swap, general frame reclamation,
block/storage stack, networking, users/groups/security database, capability
manager, GUI/desktop, APIC/x2APIC, SMP, NUMA or multicore scheduling.

## 33. Future compatibility

| Future work | Compatibility reason |
|---|---|
| multithreaded process | ProcessId/address space independent of per-thread TaskId and kernel stack |
| System Monitor | stable snapshot, PID, ticks and mapped-page count |
| Service Manager / PID 1 | generation-safe parent links can be reassigned for orphans |
| wait/reap syscall | Zombie retains result until internal reap |
| VFS/FDs 0.0.10 | resource domain is process-owned, scheduler stays unaware |
| resource handles/capabilities | opaque IDs and narrow ABI keep pointers/internals out of userspace |
| shared memory/IPC | mapping ownership can later represent shared references |
| frame reclamation | ownership metadata exists although allocator remains monotonic |
| SMP | explicit per-task ownership helps, but locks/TLB shootdowns/AP bring-up must change |

## 34. Implementation status and remaining phases

Implementation phases A through L are present on the 0.0.9 branch: bounded
identity/lifecycle, audited inactive roots, stack/RSP0 support, checked ELF,
prepared tasks/publication, CR3 switching, CPL3 PIT preemption, getpid/exit,
checked user-copy, fault containment/Zombie reaping, accounting/QEMU scenarios,
and version/docs updates. The remaining project gates are:

* **M:** separate implementation PR and human review.
* **N:** formal pre-release audit against the exact merged main and artifacts.
* **O:** RC, published-artifact verification, then stable release only after
  review. No RC or release is part of this implementation branch.

The dependency order remains significant: runtime mapping audit precedes CR3
activation, and the non-returning exception handoff precedes claims of user
fault containment.

## 35. Open questions / blockers

No unresolved correctness blocker is known from the implementation and local
QEMU review. Keep these constraints visible in future work:

* Re-run the dynamic boot-mapping audit whenever bootloader/configuration or
  required kernel mappings change; process-root activation fails closed if
  the runtime inventory cannot prove the required supervisor mappings.
* AddressSpace roots and mappings are torn down from process metadata, but
  the monotonic physical-frame allocator does not reclaim process backing
  frames or intermediate table frames. This is an intentional 0.0.9 limit.
* User-copy safety relies on single-core execution, IF-clear syscalls,
  immutable Alive mappings, no user remapping, no user threads and no SMP.
  mmap, concurrent threads or SMP require a stronger pin/locking/fault-safe
  copy design before they are added.
* Eight process slots share the existing eight scheduler Task slots with
  kernel Tasks; process table capacity does not imply eight simultaneously
  runnable user processes.
* The embedded static smoke ELF is a temporary provider until VFS/initrd
  supplies bytes; the loader API remains byte-slice based.

The `int 0x80` process ABI remains experimental. No ABI compatibility promise
is made before 0.1.0.
