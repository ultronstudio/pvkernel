# Architecture

This document describes pvKernel's current architecture as implemented in
version 0.0.4. It does not describe planned or aspirational architecture; see
[ROADMAP.md](ROADMAP.md) for that.

## Components

```
UEFI firmware (OVMF)
        │
        ▼
rust-osdev bootloader (external crate, not part of pvKernel)
        │  loads the pvKernel ELF, exits UEFI boot services,
        │  builds an initial page table mapping, and passes BootInfo
        ▼
pvKernel ELF (kernel_main)
        │
        ├── drivers::serial   – COM1 UART diagnostics (kprint!/kprintln!)
        ├── arch::x86_64      – GDT, TSS, IDT, exception + IRQ handlers, PIC, PIT
        ├── graphics::framebuffer – centered blue "P" logo
        └── halt              – fatal halt vs. interrupt-enabled idle loop
```

## Responsibility boundary

- **UEFI firmware**: powers on the machine, initializes basic platform
  devices, and runs the boot manager.
- **rust-osdev bootloader**: locates and loads the pvKernel ELF binary,
  establishes an initial 4-level page table mapping and stack, collects the
  boot memory map and framebuffer information into `BootInfo`, and transfers
  control to `kernel_main`. pvKernel does not currently own or replace this
  page table mapping.
- **pvKernel**: everything from `kernel_main` onward. This includes serial
  diagnostics, the GDT/TSS, the IDT and its exception/IRQ handlers, the
  legacy 8259 PIC and PIT drivers, and framebuffer rendering.
- **Future pvOS userspace**: not present in 0.0.4. Ring 3 execution, system
  calls, and userspace loading are future roadmap items (see
  [ROADMAP.md](ROADMAP.md)); pvKernel does not currently support them.

## Initialization order

1. `kernel_main` receives `BootInfo` from the bootloader.
2. Serial diagnostics are printed (COM1, `drivers::serial`).
3. The boot memory map is summarized.
4. `arch::init()` runs, in this exact order:
   1. Maskable hardware interrupts are disabled
      (`x86_64::instructions::interrupts::disable()`).
   2. The kernel-owned GDT and TSS are built and loaded
      (`arch::x86_64::gdt`), including the dedicated double-fault IST stack.
   3. The kernel-owned IDT is built and loaded (`arch::x86_64::interrupts`),
      including handlers for all 16 remapped IRQ vectors (32-47). This must
      happen after the GDT/TSS, because the double-fault entry references an
      IST index that is only meaningful once the TSS is active.
   4. The legacy 8259 PIC is remapped to vectors 32-47
      (`arch::x86_64::pic::initialize`), then every IRQ line is masked
      (`pic::mask_all`).
   5. PIT channel 0 is configured as a periodic timer at approximately
      100 Hz (`arch::x86_64::pit::initialize`).
5. The framebuffer is rendered (`graphics::framebuffer`), unchanged from
   0.0.1: dark background, centered blue `P` (`#00B7EF`).
6. A controlled `int3` breakpoint test verifies the IDT is functioning and
   execution resumes correctly.
7. A boot-time timer verification unmasks PIT IRQ0, enables maskable
   hardware interrupts, and confirms the tick counter advances (see
   [docs/architecture/timer.md](docs/architecture/timer.md)).
8. If the `fault-tests` Cargo feature is enabled, a deliberate page fault is
   triggered and matched against its expected faulting address.
9. The CPU enters an interrupt-enabled idle loop (`halt::idle_loop`), unless
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

- a 64-bit kernel code segment descriptor;
- a TSS descriptor, wrapping a kernel-owned `TaskStateSegment`;
- a dedicated double-fault stack (5 × 4 KiB), referenced by IST index
  `DOUBLE_FAULT_IST_INDEX`.

The GDT and TSS are stored in `spin::Once` cells with `'static` storage
duration, loaded once, and never replaced during the kernel's lifetime.

## IDT ownership

`arch::x86_64::interrupts` owns a kernel-owned `InterruptDescriptorTable`,
also stored in a `spin::Once` cell, installed after the GDT/TSS. See
[docs/architecture/interrupts.md](docs/architecture/interrupts.md) for
handler-level detail.

## Fatal exception behavior

Breakpoint exceptions log the event and return normally. Invalid opcode,
general protection fault, page fault, and double fault are treated as fatal:
they log structured diagnostics (exception name, error code where relevant,
faulting address for page faults, and the interrupt stack frame) and then
call `halt::halt_forever` directly (or, under the `qemu-test` feature, exit
QEMU with a failure status instead), without invoking `panic!`. The one
exception is a deliberately triggered page fault under the `fault-tests`
feature: when it matches the expected faulting address, it is treated as a
passed test rather than a fatal condition (see
[docs/development/testing.md](docs/development/testing.md)).
