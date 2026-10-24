# pvKernel 0.0.8 kernel task and scheduler design

## Scope and inspected baseline

This design is for pvKernel 0.0.8 only: single CPU, ring-0 kernel tasks,
bounded preemptive round-robin scheduling. It does not add processes, user
task scheduling, CR3 switching, blocking, or SMP.

The implementation baseline is the stable v0.0.7 commit
`1621819015def0b1cca8d7b702eab1831e5a67b8`. Inspection of
`kernel/src/arch/x86_64/interrupts.rs`, `pit.rs`, `pic.rs`, `gdt.rs`,
`syscall/entry.rs`, and `userspace/` established these current semantics:

* PIT IRQ0 is remapped to IDT vector 32 (`IrqLine::Timer.vector()`). The
  current entry is a Rust `extern "x86-interrupt"` function. It increments
  `pit::TICKS` (`AtomicU64`, relaxed) and sends one master-PIC EOI.
* `set_handler_fn` installs an interrupt gate (the IDT default); on entry,
  hardware clears IF. Thus maskable IRQ nesting is already disabled in the
  handler. The handler itself does not enable IF, allocate, log, format, or
  lock. A replacement scheduler entry must retain these properties.
* The long-mode interrupt frame observed by the existing
  `InterruptStackFrame` path and confirmed by QEMU's synthetic-context probe
  includes `RIP`, `CS`, `RFLAGS`, `RSP`, and `SS`. A CPL3-to-CPL0 interrupt
  additionally switches to TSS.RSP0, while same-CPL interrupts use the
  current stack. QEMU confirmed `iretq` consumes the RSP/SS words for a
  synthetic CPL0 return: omitting them faults at the stack top. A dedicated
  entry still owns the saved context explicitly and avoids the Rust
  handler's compiler-managed frame.
* CPL3 payloads run with PIT IRQ0 enabled. The syscall test observes a tick
  that interrupted CPL3 via a relaxed atomic set by the timer handler. A
  kernel scheduler must therefore inspect saved `CS.RPL` and skip scheduling
  on CPL3-origin IRQs while still recording the tick and sending EOI.
* The PIC's EOI helper writes the appropriate PIC command port. For IRQ0 it
  sends one master EOI. EOI is currently sent after recording the tick.
* The kernel has no task scheduler yet. It initializes the GDT/TSS and IDT
  with interrupts disabled, then validates paging, initializes the frame
  allocator/page-table access and heap, then the PIC/PIT. `kernel_main`
  enables IF and waits on PIT ticks before entering a `hlt` idle loop.
* `without_interrupts` from `x86_64` saves/restores the prior IF state;
  nesting is safe because an inner call observes IF=0 and does not re-enable
  interrupts on return. Scheduler/task state APIs must use it around their
  spin lock. The heap allocator and page-table access already do this.
* Serial output uses a `try_lock` and skips output when contended; it does not
  spin while holding the serial lock. It is not suitable for the IRQ hot path.
  Framebuffer boot output has no persistent scheduler-facing lock. Heap and
  page-table critical sections mask interrupts.
* `x86_64-unknown-none` for the pinned nightly reports `disable-redzone: true`
  and features `-mmx,-sse,-sse2,-sse3,-ssse3,-sse4.1,-sse4.2,-avx,-avx2,+soft-float`.
  The Rust ABI is soft-float. This provides the 0.0.8 FPU/SIMD policy: kernel
  task code is compiled without compiler-generated FPU/SIMD use. Do not add
  target features that enable it without adding eager state preservation.
* The target's 16-byte stack alignment is enforced at the Rust call boundary.
  Synthetic task contexts enter an assembly trampoline which makes an ABI
  call with a 16-byte-aligned pre-call RSP. The trampoline owns task return.
* The kernel uses `RFLAGS` as an interrupt-return image. Initial contexts use
  reserved bit 1 set, IF set, and clear IOPL, DF, and TF. Interrupts remain
  disabled while scheduler bookkeeping and the actual context selection run;
  IF becomes enabled only as part of restoring a task's frame with `iretq`.

These findings rule out switching by returning from the current
`extern "x86-interrupt"` Rust handler: its compiler-managed frame and the
different stack source for CPL0/CPL3 hardware frames make an arbitrary stack
swap there unsafe. The scheduler design uses a dedicated IRQ0 assembly entry that saves
the machine state in one documented format, calls a small Rust policy
function with an ABI-aligned stack, and restores the selected frame with
`iretq`. Other exception and IRQ handlers remain unchanged.

## Task IDs, states, and storage

`TaskId` is a checked pair of a bounded slot index and a nonzero `u64`
generation. It is never a pointer. Every slot has a generation counter. On
reuse, generation advances with `checked_add`; a slot whose generation would
overflow is permanently retired. Lookup validates the index, generation,
and slot state before accessing a TCB. Old IDs return a stale/invalid-ID
error.

The task table and ready queue are fixed-size arrays with capacity
`MAX_KERNEL_TASKS = 8`. Eight tasks are sufficient for the scheduler tests
and early kernel tasks while keeping static metadata and pre-mapped stack
memory bounded. The TCB stores ID, lifecycle state, saved context pointer,
stack slot, and queue-membership flag. The initial entry pointer lives in
the synthetic RDI register until the assembly trampoline consumes it. No
scheduler metadata operation allocates.

States are `Vacant`, `Ready`, `Running`, `Exiting`, `Exited`, and `Retired`.
Exactly one ordinary task can be `Running`; the bootstrap context is a
separate scheduler-owned context and is the idle fallback. Ready tasks are
queued exactly once. A Running, Exiting, Exited, Vacant, or Retired task is
not queued. Invalid queue/state relationships are kernel-fatal invariants.

## Task stacks and ownership

The reserved virtual region begins at `0xffff_9000_2000_0000`. Each of the
eight slots occupies six 4 KiB pages: one unmapped lower guard, four mapped
stack pages (16 KiB), and one unmapped upper guard. Region end and every slot
address are calculated with checked arithmetic and canonical/page-alignment
validation. The whole region is checked against existing mappings before
mapping; any collision or mapping failure aborts scheduler initialization.
This range is separate from the v0.0.7 temporary mapping probe at
`0xffff_9000_0000_0000`, the 1 MiB heap at `0xffff_9000_1000_0000`, and the
low userspace test range at `0x0000_6000_0000_0000`; bootloader-selected
mappings are still checked at runtime.

Stacks grow down. The low-address guard is the overflow boundary; the
high-address guard prevents accidental use beyond the initial top. Initial
RSP is the exclusive high end of the mapped pages, aligned to 16 bytes before
the synthetic interrupt frame is placed. Every mapped page must translate
as present, writable, supervisor-only, and NX; both guards must translate as
not mapped. These checks inspect actual page-table translations after
mapping.

All stack slots are pre-mapped during scheduler initialization, before IRQ0
can schedule tasks. Their physical frames transfer to the task-stack
subsystem and remain owned for kernel lifetime; slot reuse reuses those same
backing pages. Reusing a slot first clears its mapped stack bytes before the
new task context is published. No frames are returned to the monotonic boot
allocator.

## Context frame and switch boundary

Preemption occurs only at the PIT IRQ0 return boundary. The timer assembly
entry saves all 15 general-purpose registers in a fixed software prefix,
followed by the hardware frame. With the saved-context pointer at the lowest
address, the intended layout is:

```text
low addresses / saved RSP
  r15, r14, r13, r12, r11, r10, r9, r8
  rdi, rsi, rbp, rdx, rcx, rbx, rax
  RIP
  CS
  RFLAGS
  RSP
  SS
high addresses
```

The precise push order and offsets in the assembly implementation are the
source of truth and must match named constants / compile-time layout checks.
In the inspected long-mode configuration `iretq` consumes all five hardware
frame words for CPL0 and CPL3 returns. CPL3-origin IRQs use the existing
no-switch return path and their frame is preserved untouched. No CPL3 frame
is interpreted as a kernel task context. Synthetic CPL0 contexts include
explicit RSP and the kernel's null SS value; initial RSP points at the
aligned stack top.

Before calling Rust, assembly retains the interrupted saved-context pointer,
aligns a temporary call stack to the SysV x86_64 16-byte ABI, and calls only
the allocation-free scheduler decision function. The Rust call returns the
selected context pointer. The lock has been released before assembly adopts
that pointer. IRQ0 EOI is sent exactly once after tick recording and the
scheduler decision, before restoring the selected context. IF remains clear
until `iretq` restores the selected task's flags. Assembly executes `cld`
before calling Rust to satisfy the ABI direction-flag invariant; `iretq`
restores the interrupted task's original DF value from RFLAGS.

Initial task contexts contain defined zero values for all GPRs, the kernel
code selector, a deliberate safe RFLAGS image, and the task-entry assembly
trampoline as RIP, the trusted `fn()` entry pointer in RDI, the aligned stack
top in RSP, and the kernel's null SS value. The trampoline makes the Rust
call with correct ABI stack alignment. If the entry returns, it calls the
non-returning task-exit path; it never returns into a fabricated or
uninitialized address.

The task stack uses `iretq` to enter the trampoline. The trampoline then
establishes ordinary Rust call alignment explicitly. Task code returning
marks its task `Exiting` with interrupts masked and waits for the next PIT
IRQ to switch away. Only after the IRQ has saved the exiting frame and
selected another context is that task marked `Exited`, proving no instruction
continues on its stack. The IRQ gate keeps IF clear through the assembly
restore, so no ordinary spawn API can run between marking Exited and leaving
that stack. A later spawn can reuse the Exited slot and stack; no task can
reuse its own active stack.

## Queue and scheduling policy

The runnable queue is a fixed-size ring of task slot/generation identities.
Each TCB's `queued` flag prevents duplicate insertion in O(1); dequeue
validates ID generation and Ready state. Capacity is bounded by eight and
normal enqueue/dequeue is O(1). Queue corruption is fatal, while spawning
when all slots are occupied returns deterministic capacity exhaustion.
Wrap-around is explicitly covered by tests.

Round-robin uses a one-tick quantum at the existing approximately 99 Hz PIT
rate. If the current task remains runnable at IRQ0, it transitions to Ready
and is enqueued once, then the next Ready task is selected. If no Ready task
exists, bootstrap/idle is selected. A task is not switched to itself when
there is no other runnable work. The tick counter is not reset by scheduler
initialization.

## Bootstrap, idle, locks, and timer behavior

The pre-scheduler boot execution is a special bootstrap/idle context, not a
spawned task and not a queue member. Its first scheduler-managed timer IRQ
saves its actual stack/context; if there is no runnable work, the same
context resumes. After boot checks and task smoke tests, idle uses `hlt`; a
PIT interrupt wakes it. While ordinary tasks exist, timer IRQs round-robin
between their saved contexts and bootstrap. There is no busy-spin idle loop.

Scheduler state APIs mask interrupts before acquiring the scheduler spin
lock, and restore the prior IF state on return. This prevents a task from
being interrupted while holding that lock and deadlocking in IRQ context.
IRQ policy runs with IF already clear due to the interrupt gate. The lock is
released before context selection is handed to assembly. No lock guard,
allocator call, logging, blocking, page mapping, or unbounded scan crosses
the switch boundary.

Kernel code must mask interrupts around any scheduler-unsafe spin lock held
across arbitrary work, because a timer preemption while holding such a lock
could let another task deadlock on the same single CPU. Existing heap and
page-table locks already mask interrupts. Serial uses nonblocking `try_lock`
and drops output under contention. Additions must follow the same rule.

IRQ0 handling increments the existing monotonic tick once, optionally
records the existing CPL3 test observation, makes a scheduling decision only
for CPL0-origin interrupts, sends exactly one PIC EOI, and restores the
selected interrupt frame. The scheduler does not enable interrupts itself.
No scheduler task is scheduled from a CPL3 IRQ. The existing provisional
`int 0x80` path and TSS.RSP0 remain unchanged.

## Failure, panic, and limitations

Task table exhaustion is a normal deterministic spawn error. Invalid
virtual ranges, mapping failures, wrong stack flags, guard collisions,
invalid contexts, and state/queue corruption are fatal kernel invariants.
Panic remains kernel-fatal because panic strategy is abort; there is no
per-task unwind/recovery.

Limitations: one CPU, 8259 PIC/PIT only, eight fixed task slots, 16 KiB
fixed-size stacks, fixed virtual stack region, no stack growth, no blocking,
sleep, wait queues, join, priorities, user-task scheduling, CR3 switching,
SMP, or physical-frame reclamation. Compiler-generated FPU/SIMD is disabled
for this target; enabling it requires eager context preservation first.
