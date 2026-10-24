# Architecture

This document describes pvKernel's current architecture as implemented in
version 0.0.9. It does not describe planned or aspirational architecture; see
[ROADMAP.md](ROADMAP.md) for that.

## Components

```text
UEFI firmware (OVMF)
    |
rust-osdev bootloader
    |  upper-half dynamic mapping arena, BootInfo, framebuffer, memory map
    v
pvKernel kernel_main
    |-- drivers::serial, graphics::framebuffer, halt
    |-- arch::x86_64
    |   |-- GDT/TSS/RSP0, IDT, PIC/PIT, exception and timer entry
    |   |-- memory: physical allocator, audited kernel root, AddressSpace
    |   |-- syscall: experimental scalar int 0x80 ABI
    |-- task: bounded scheduler, kernel/user Task contexts, SwitchPlan
    |-- process: ProcessId/PID table, lifecycle, ELF64 loader, snapshots
```
## Responsibility boundary

The root `task` module owns the bounded Task table, runnable ring, context
selection, and kernel/user execution-context state. `process` owns generation-
safe ProcessId identity, boot-lifetime PIDs, process lifecycle, AddressSpace
ownership, and process metadata. `arch::x86_64::memory` owns the kernel root,
inactive process-root construction, mapping audit, and CR3 activation.
`arch::x86_64::task` owns task-stack mapping, the saved frame layout, and the
timer-entry/trampoline assembly. Scheduler policy remains separate from
process policy, and the PIT hot path does not allocate or parse executable data.

* **UEFI firmware**: powers on the machine, initializes basic platform
  devices, and runs the boot manager.
* **rust-osdev bootloader**: loads the pvKernel ELF, exits UEFI boot services,
  creates the initial 4-level mappings and stack, and passes `BootInfo`.
  pvKernel audits required mappings and constrains dynamic boot mappings to
  the upper canonical half before it can activate process CR3 roots.
* **Kernel Tasks**: bounded preemptive single-core ring-0 contexts with
  dedicated supervisor-only guarded stacks.
* **Process Tasks**: one CPL3 Task per process in 0.0.9, each with an isolated
  lower-half address space, dedicated USER RW/NX stack and supervisor kernel
  entry stack. Timer IRQs can preempt CPL3 execution and switch CR3/RSP0.
* **Userspace ABI**: experimental `int 0x80` scalar `getpid` and terminal
  `exit`. The checked static ELF64 loader accepts only validated `ET_EXEC`
  `PT_LOAD` images. CPL3 #PF/#GP/#UD terminate the affected process and retain
  scalar fault metadata. No stable ABI is promised before 0.1.0.
* **Future work**: VFS/initrd, `/bin/init`, FDs, fork/general exec, dynamic
  linking, user heap/mmap, signals, IPC, general frame reclamation, storage,
  networking, SMP, and GUI remain out of scope.
## Initialization order

1. `kernel_main` receives `BootInfo` from the bootloader.
2. Serial diagnostics are printed (COM1, `drivers::serial`).
3. The boot memory map is summarized.
4. The framebuffer is taken out of `BootInfo` (`boot_info.framebuffer.take()`),
   so `kernel_main` still has it after `BootInfo` itself is consumed in the
   next step.
5. `arch::init(boot_info)` runs, taking `boot_info` by value (not `&mut`):
   nothing in the kernel can reach `BootInfo` again after this call returns.
   It runs, in this exact order:
   1. Maskable hardware interrupts are disabled
      (`x86_64::instructions::interrupts::disable()`).
   2. The kernel-owned GDT and TSS are built and loaded
      (`arch::x86_64::gdt`), including the dedicated double-fault IST stack,
      ring-3 (CPL3) code/data segment descriptors, and a kernel-lifetime
      fallback TSS.RSP0 stack. User Task kernel-stack tops are installed
      before returning to CPL3 (see
      [docs/architecture/processes.md](docs/architecture/processes.md)).
   3. The kernel-owned IDT is built and loaded (`arch::x86_64::interrupts`),
      including handlers for all 16 remapped IRQ vectors (32-47) and the
      `int 0x80` syscall gate (see
      [docs/architecture/syscalls.md](docs/architecture/syscalls.md)). This must
      happen after the GDT/TSS, because the double-fault entry references an
      IST index that is only meaningful once the TSS is active.
   4. The physical frame allocator and active page-table access are
      initialized (`arch::x86_64::memory::init`, which receives `boot_info`
      and consumes it completely). This must happen after the IDT, so a
      page fault occurring during memory initialization is still handled,
      and before the PIC/PIT, so no other subsystem can create page-table
      mappings first. Internally, this step validates that CR4.LA57
      (five-level paging) is **not** active and that IA32_EFER.NXE is
      enabled, then enables and verifies CR0.WP, before doing anything that
      assumes a four-level CR3 root. It then takes ownership of
      `BootInfo::memory_regions` (moving it out, rather than aliasing it)
      before building the frame allocator from it. See
      [docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md).
   5. The legacy 8259 PIC is remapped to vectors 32-47
      (`arch::x86_64::pic::initialize`), then every IRQ line is masked
      (`pic::mask_all`).
   6. PIT channel 0 is configured as a periodic timer at approximately
      100 Hz (`arch::x86_64::pit::initialize`).
5. A boot-time memory smoke test verifies the mapping foundation (map,
   translate, write/read, change permissions, unmap, re-translate); see
   [docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md)
   and
   [docs/development/memory-testing.md](docs/development/memory-testing.md).
6. The framebuffer is rendered (`graphics::framebuffer`), unchanged from
   0.0.1: dark background, centered blue `P` (`#00B7EF`).
7. A controlled `int3` breakpoint test verifies the IDT is functioning and
   execution resumes correctly.
8. `task::init` reserves and verifies task stacks. Two finite smoke tasks
   are created, but not run yet, before IRQ0 is unmasked.
9. PIT IRQ0 is unmasked, maskable interrupts are enabled, the two tasks run
   under preemption and return to bootstrap, and timer progress is confirmed
   (see [docs/architecture/timer.md](docs/architecture/timer.md)).
10. If the `fault-tests` Cargo feature is enabled, a deliberate page fault is
   triggered and matched against its expected faulting address.
11. If the `mem-fault-tests` Cargo feature is enabled, a temporary page is
    mapped, written to, unmapped, and deliberately re-accessed, matching the
    resulting page fault against its expected address (see
    [docs/development/memory-testing.md](docs/development/memory-testing.md)).
12. If `supervisor-write-protection-test` is enabled, a CPL0 write to a
    supervisor-only read-only page verifies CR0.WP behavior through an
    expected protection-violation page fault.
13. The process subsystem audits required boot mappings, initializes its
    bounded table, and can construct inactive process address spaces while
    preserving audited supervisor-only kernel mappings.
14. Process-owned tasks use checked ELF loading, dedicated user and kernel
    stacks, and timer-driven CPL3 preemption. On ordinary boots an embedded
    static smoke ELF exercises CPL3 entry, `getpid`, and terminal `exit`;
    QEMU features select deterministic process and legacy boundary scenarios.
15. The timer entry saves the complete interrupt context and can schedule
    both CPL0 kernel tasks and process-owned CPL3 tasks, switching CR3 and
    TSS.RSP0 according to the selected context.
16. The CPU enters an interrupt-enabled idle loop (`halt::idle_loop`), unless
    the `qemu-test` Cargo feature is enabled, in which case QEMU exits
    deterministically instead (see
    [docs/development/testing.md](docs/development/testing.md)).

## Hardware interrupts in 0.0.4

0.0.4 adds a legacy 8259 PIC driver and a PIT-based timer, and safely enables
maskable hardware interrupts for the first time. Every IRQ line has an IDT
handler (so no vector is ever left unhandled), but only PIT IRQ0 is ever
unmasked; all other lines remain masked and, if a spurious or unexpected
interrupt arrives on one of them, it is acknowledged and recorded rather than
crashing the kernel. See [docs/architecture/hardware-interrupts.md](docs/architecture/hardware-interrupts.md)
and [docs/architecture/timer.md](docs/architecture/timer.md) for detail.
APIC support, keyboard input, and any other IRQ line remain future roadmap
items.

## GDT/TSS ownership

`arch::x86_64::gdt` owns:

* a 64-bit kernel code segment descriptor;
* a TSS descriptor, wrapping a kernel-owned `TaskStateSegment`;
* a dedicated double-fault stack (5 × 4 KiB), referenced by IST index
  `DOUBLE_FAULT_IST_INDEX`;
* ring-3 (CPL3) code and data segment descriptors, and a kernel-lifetime
  fallback RSP0 stack for bootstrap/kernel execution. Before returning to
  CPL3, the scheduler installs the selected user Task's guarded supervisor
  kernel-stack top in TSS.RSP0 (see
  [docs/architecture/processes.md](docs/architecture/processes.md)).

The GDT and TSS are stored in `spin::Once` cells with `'static` storage
duration, loaded once, and never replaced during the kernel's lifetime.

## IDT ownership

`arch::x86_64::interrupts` owns a kernel-owned `InterruptDescriptorTable`,
also stored in a `spin::Once` cell, installed after the GDT/TSS. See
[docs/architecture/interrupts.md](docs/architecture/interrupts.md) for
handler-level detail.

## Memory ownership

`arch::x86_64::memory` owns:

* `BootFrameAllocator` — a monotonic physical frame allocator built once
  from the bootloader's memory map;
* the active kernel root (`KernelAddressSpace`) and process-owned inactive
  `AddressSpace` roots. Page-table frames are reached through the audited
  bootloader physical-memory offset mapping and paired with the frame
  allocator.

Both are stored together behind a `spin::Mutex`, itself behind a
`spin::Once`, and are reachable only through
`memory::with_kernel_address_space`, which disables maskable hardware
interrupts for the duration of the call. This mirrors the GDT/TSS and IDT's
`spin::Once`-backed, install-once ownership pattern above. See
[docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md)
for the full design, including the safety invariants for reconstructing the
active page table and the permission (W^X) model enforced by every mapping
operation.

## Kernel heap ownership (0.0.7)

`arch::x86_64::memory::heap` maps the fixed 1 MiB heap after paging
prerequisites and the active page table are validated. It verifies that
every heap page and both adjacent guard pages were unmapped, creates
supervisor-only writable NX mappings, verifies representative permissions,
then initializes the `#[global_allocator]`. Its first-fit bitmap reuses heap
object storage; adjacent free allocation units naturally combine into larger
runs. Backing physical frames remain owned by
the heap for the kernel lifetime and are not reclaimed by `Box`, `Vec`, or
`String` deallocation. See
[docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md)
for the layout and safety invariants.

## Syscall and userspace entry ownership (0.0.9)

`arch::x86_64::syscall` owns the experimental scalar `int 0x80` ABI: a
naked-asm entry stub at vector `0x80`, the `SyscallFrame` saved on the
selected user's supervisor RSP0 stack, and a Rust dispatcher for `getpid`
and terminal `exit`. The separate legacy `arch::x86_64::userspace` module
retains bootstrap CPL3 boundary-test payloads. Process-owned user Tasks use
the normal checked ELF loader and synthetic scheduler context; ordinary
boots run an embedded static smoke executable until VFS/initrd is available.
See
[docs/architecture/syscalls.md](docs/architecture/syscalls.md) and
[docs/architecture/processes.md](docs/architecture/processes.md) for the
current details.

## Fatal exception behavior

Breakpoint exceptions log the event and return normally. CPL0 invalid opcode,
general protection fault, page fault, and double fault remain kernel-fatal:
they log structured diagnostics (exception name, error code where relevant,
faulting address for page faults, the CPL the fault originated from, and the
interrupt stack frame) and then call `halt::halt_forever` directly (or,
under the `qemu-test` feature, exit QEMU with a failure status instead),
without invoking `panic!`. The exceptions are deliberately triggered faults
under the `fault-tests`, `userspace-privilege-fault-test`, and
`userspace-supervisor-fault-test` features: when one matches its expected
signature, it is treated as a passed test rather than a fatal condition.
CPL3 #UD/#GP/#PF from a process-owned Task instead capture scalar fault
metadata and use the terminal scheduler handoff to terminate that process;
the kernel remains active. See
[docs/development/testing.md](docs/development/testing.md) and
[docs/development/userspace-testing.md](docs/development/userspace-testing.md).
