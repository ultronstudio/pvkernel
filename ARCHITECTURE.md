# Architecture

This document describes pvKernel's current architecture as implemented in
version 0.0.2. It does not describe planned or aspirational architecture; see
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
        ├── arch::x86_64      – GDT, TSS, IDT, exception handlers
        ├── graphics::framebuffer – centered blue "P" logo
        └── halt              – CPU halt loop
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
  diagnostics, the GDT/TSS, the IDT and its exception handlers, and
  framebuffer rendering.
- **Future pvOS userspace**: not present in 0.0.2. Ring 3 execution, system
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
   3. The kernel-owned IDT is built and loaded (`arch::x86_64::interrupts`).
      This must happen after the GDT/TSS, because the double-fault entry
      references an IST index that is only meaningful once the TSS is
      active.
5. The framebuffer is rendered (`graphics::framebuffer`), unchanged from
   0.0.1: dark background, centered blue `P` (`#00B7EF`).
6. A controlled `int3` breakpoint test verifies the IDT is functioning and
   execution resumes correctly.
7. The CPU halts (`halt::halt_loop`).

## Why hardware interrupts remain disabled in 0.0.2

0.0.2 only implements CPU exception handling (faults raised by the CPU
itself, such as page faults or invalid opcodes). It does not implement a
PIC/APIC or any external interrupt controller. Enabling maskable hardware
interrupts without a controller and IRQ handlers configured would allow
unhandled interrupts to reach the CPU, which is out of scope for this
release. Hardware interrupt support is planned for 0.0.3.

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
halt the CPU directly, without invoking `panic!`.
