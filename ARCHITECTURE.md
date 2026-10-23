# Architecture

This document describes pvKernel's current architecture as implemented in
version 0.0.6. It does not describe planned or aspirational architecture; see
[ROADMAP.md](ROADMAP.md) for that.

## Components

```
UEFI firmware (OVMF)
        │
        ▼
rust-osdev bootloader (external crate, not part of pvKernel)
        │  loads the pvKernel ELF, exits UEFI boot services,
        │  builds an initial page table mapping (incl. a physical-memory
        │  offset mapping), and passes BootInfo
        ▼
pvKernel ELF (kernel_main)
        │
        ├── drivers::serial   – COM1 UART diagnostics (kprint!/kprintln!)
        ├── arch::x86_64      – GDT, TSS, IDT, exception + IRQ handlers, PIC, PIT
        ├── arch::x86_64::memory – physical frame allocator, active page table
        │                          access, map/unmap/permission operations
        ├── arch::x86_64::syscall – provisional int 0x80 syscall ABI
        ├── arch::x86_64::userspace – test-only CPL0 → CPL3 (iretq) entry
        ├── graphics::framebuffer – centered blue "P" logo
        └── halt              – fatal halt vs. interrupt-enabled idle loop
```

## Responsibility boundary

* **UEFI firmware**: powers on the machine, initializes basic platform
  devices, and runs the boot manager.
* **rust-osdev bootloader**: locates and loads the pvKernel ELF binary,
  establishes an initial 4-level page table mapping and stack, collects the
  boot memory map and framebuffer information into `BootInfo`, and transfers
  control to `kernel_main`. pvKernel does not currently own or replace this
  page table mapping.
* **pvKernel**: everything from `kernel_main` onward. This includes serial
  diagnostics, the GDT/TSS, the IDT and its exception/IRQ handlers, the
  legacy 8259 PIC and PIT drivers, the physical frame allocator and active
  page-table access, a provisional `int 0x80` syscall ABI, and (test-only)
  CPL0 → CPL3 userspace entry, and framebuffer rendering.
* **Future pvOS userspace**: 0.0.6 adds only a minimal, test-only CPL0 →
  CPL3 transition and bootstrap syscall ABI (see
  [docs/architecture/userspace-entry.md](docs/architecture/userspace-entry.md)
  and [docs/architecture/syscalls.md](docs/architecture/syscalls.md)); a
  process model, scheduler, and ELF userspace loading remain future
  roadmap items (see [ROADMAP.md](ROADMAP.md)); pvKernel does not
  currently support them.

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
      ring-3 (CPL3) code/data segment descriptors, and a dedicated
      TSS.RSP0 ring-transition kernel stack (see
      [docs/architecture/userspace-entry.md](docs/architecture/userspace-entry.md)).
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
8. A boot-time timer verification unmasks PIT IRQ0, enables maskable
   hardware interrupts, and confirms the tick counter advances (see
   [docs/architecture/timer.md](docs/architecture/timer.md)).
9. If the `fault-tests` Cargo feature is enabled, a deliberate page fault is
   triggered and matched against its expected faulting address.
10. If the `mem-fault-tests` Cargo feature is enabled, a temporary page is
    mapped, written to, unmapped, and deliberately re-accessed, matching the
    resulting page fault against its expected address (see
    [docs/development/memory-testing.md](docs/development/memory-testing.md)).
11. If `supervisor-write-protection-test` is enabled, a CPL0 write to a
    supervisor-only read-only page verifies CR0.WP behavior through an
    expected protection-violation page fault.
12. If exactly one `userspace-*-test` Cargo feature is enabled, a hand-written
    test payload is mapped into the reserved userspace range and entered at
    CPL3 via `iretq`; the scenario ends deterministically (see
    [docs/development/userspace-testing.md](docs/development/userspace-testing.md)).
13. The CPU enters an interrupt-enabled idle loop (`halt::idle_loop`), unless
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
* ring-3 (CPL3) code and data segment descriptors, and a dedicated,
  writable TSS.RSP0 ring-transition kernel stack (4 × 4 KiB), used only
  when entering CPL3 (see
  [docs/architecture/userspace-entry.md](docs/architecture/userspace-entry.md)).

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
* `KernelAddressSpace` — the single active x86_64 page table (an
  `OffsetPageTable` over the CR3-resident level-4 table, reached through the
  bootloader's physical-memory offset mapping), paired with the frame
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

## Syscall and userspace entry ownership (0.0.6)

`arch::x86_64::syscall` owns a minimal, provisional, scalar-only `int 0x80`
syscall ABI: a naked-asm entry stub installed into the IDT's vector `0x80`,
a `SyscallFrame` register-frame layout built on the RSP0 stack, and a
Rust-side dispatcher. `arch::x86_64::userspace` owns the (test-only, behind
Cargo features) code that maps a hand-written payload into the reserved
userspace virtual-address range and transitions into it via `iretq`. See
[docs/architecture/syscalls.md](docs/architecture/syscalls.md) and
[docs/architecture/userspace-entry.md](docs/architecture/userspace-entry.md)
for the full design.

## Fatal exception behavior

Breakpoint exceptions log the event and return normally. Invalid opcode,
general protection fault, page fault, and double fault are treated as fatal:
they log structured diagnostics (exception name, error code where relevant,
faulting address for page faults, the CPL the fault originated from, and the
interrupt stack frame) and then call `halt::halt_forever` directly (or,
under the `qemu-test` feature, exit QEMU with a failure status instead),
without invoking `panic!`. The exceptions are deliberately triggered faults
under the `fault-tests`, `userspace-privilege-fault-test`, and
`userspace-supervisor-fault-test` features: when one matches its expected
signature, it is treated as a passed test rather than a fatal condition (see
[docs/development/testing.md](docs/development/testing.md) and
[docs/development/userspace-testing.md](docs/development/userspace-testing.md)).
