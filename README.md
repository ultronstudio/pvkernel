# pvKernel

pvKernel is an experimental open-source x86_64 operating system kernel
written in Rust and developed as the foundation of pvOS.

> [!WARNING]
> pvKernel is in a very early stage of development. It is not currently
> suitable for production use or as a general-purpose operating system kernel.

![pvKernel boot screen](docs/images/pvkernel-0.0.1.png)

## Current milestone

Version: `0.0.4` — Hardware Interrupts and Timer

Implemented:

* UEFI boot
* x86_64 kernel entry point
* boot memory map access
* linear framebuffer access
* a kernel-owned Global Descriptor Table (GDT)
* a kernel-owned Task State Segment (TSS)
* a dedicated Interrupt Stack Table stack for double faults
* a kernel-owned Interrupt Descriptor Table (IDT)
* exception handlers for breakpoint, invalid opcode, general protection
  fault, page fault, and double fault
* a controlled breakpoint (`int3`) boot-time verification
* an optional page-fault test behind the `fault-tests` Cargo feature
* a legacy 8259 PIC driver (IRQ vector remapping, masking, EOI, spurious IRQ
  detection)
* a PIT channel 0 periodic timer with an atomic tick counter
* IDT handlers for all 16 remapped IRQ vectors, with a boot-time verification
  that maskable hardware interrupts are safely enabled and timer ticks are
  delivered
* deterministic, headless QEMU tests behind the `qemu-test` Cargo feature
* structured serial diagnostics
* panic handler
* an interrupt-enabled idle loop (plus a separate fatal halt path)

Not implemented:

* APIC initialization
* keyboard or mouse input
* physical memory allocator
* virtual memory manager
* kernel heap or dynamic allocation
* processes, threads, or a scheduler
* user mode or system calls
* ELF userspace loading
* filesystems
* networking
* device drivers beyond serial, framebuffer, PIC, and PIT

pvKernel does not claim production readiness, general hardware support,
process isolation, a stable ABI, or readiness as a reusable kernel or pvOS
foundation.

## Build

```bash
cargo build
```

## Run

```bash
cargo run
```

## Run the optional page-fault test

Deliberately dereferences an unmapped virtual address to exercise the
page-fault handler (CR2, error flags, and stack frame diagnostics), then
halts. Disabled by default; the root workspace forwards the feature to the
kernel build:

```bash
cargo run --features fault-tests
```

See [docs/development/debugging.md](docs/development/debugging.md) for the
expected output.

## Run the deterministic headless QEMU tests

Verifies PIC/PIT initialization and repeated timer interrupt delivery (and,
with `fault-tests` also enabled, an expected page fault), then exits QEMU
with a pass/fail status instead of leaving a graphical window open:

```bash
cargo run --features qemu-test -- --headless-test
cargo run --features "qemu-test fault-tests" -- --headless-test
```

See [docs/development/testing.md](docs/development/testing.md) for details.

## Current boot flow

```
UEFI firmware
    ↓
rust-osdev bootloader
    ↓
pvKernel ELF
    ↓
kernel_main
    ├── serial logging
    ├── boot memory map
    ├── x86_64 GDT, TSS and IDT
    ├── PIC remap + PIT configuration
    ├── framebuffer
    ├── controlled breakpoint test
    ├── timer interrupt verification
    └── interrupt-enabled idle loop
```

The bootloader is currently responsible only for loading the kernel and
providing initial boot information. Kernel subsystems and operating-system
services are implemented by pvKernel itself. See
[ARCHITECTURE.md](ARCHITECTURE.md) for details.

## Documentation

* [ARCHITECTURE.md](ARCHITECTURE.md) — current kernel architecture
* [docs/architecture/boot.md](docs/architecture/boot.md) — boot flow
* [docs/architecture/interrupts.md](docs/architecture/interrupts.md) —
  IDT and exception handling
* [docs/architecture/hardware-interrupts.md](docs/architecture/hardware-interrupts.md) —
  legacy 8259 PIC driver
* [docs/architecture/timer.md](docs/architecture/timer.md) — PIT timer driver
* [docs/development/build.md](docs/development/build.md) — build environment
* [docs/development/debugging.md](docs/development/debugging.md) —
  debugging and diagnostics
* [docs/development/testing.md](docs/development/testing.md) —
  deterministic headless QEMU tests
* [docs/dependencies.md](docs/dependencies.md) — direct dependency review
* [ROADMAP.md](ROADMAP.md) — planned milestones
* [CHANGELOG.md](CHANGELOG.md) — release history
* [CONTRIBUTING.md](CONTRIBUTING.md) — contribution guide
* [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) — community expectations
* [SECURITY.md](SECURITY.md) — security policy

## License

Licensed under either the MIT License or the Apache License 2.0,
at your option.

## Screenshot

![pvKernel boot screen](docs/images/pvkernel-0.0.1.png)