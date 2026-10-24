# pvKernel

pvKernel is an experimental open-source x86_64 operating system kernel
written in Rust and developed as the foundation of pvOS.

> [!WARNING]
> pvKernel is in a very early stage of development. It is not currently
> suitable for production use or as a general-purpose operating system kernel.

![pvKernel boot screen](docs/images/pvkernel-0.0.1.png)

## Current milestone

Version: `0.0.9` — Processes + Address Spaces + Userspace Foundation

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
* a monotonic physical frame allocator built from the bootloader memory map
* safe, narrowly-scoped access to the active x86_64 page table
* 4 KiB map/unmap/permission-change/translate operations with a W^X
  permission model
* a boot-time memory smoke test and an optional post-unmap page-fault test
  behind the `mem-fault-tests` Cargo feature
* required NXE and CR0.WP paging invariants, plus a deterministic CPL0
  supervisor write-protection QEMU test
* deterministic, headless QEMU tests behind the `qemu-test` Cargo feature
* dedicated ring-3 (CPL3) code/data GDT descriptors, a kernel-lifetime RSP0
  fallback, and per-user-task supervisor kernel-entry stacks
* validated synthetic CPL3 context entry via `iretq`, including both legacy
  boundary tests and process-owned userspace Tasks
* a minimal, provisional, scalar-only `int 0x80` syscall ABI
* `USER_DATA`/`USER_CODE` page permissions, reusing the existing W^X
  mapping API
* four deterministic, headless QEMU tests for the syscall boundary,
  supervisor write protection, an expected CPL3 privilege-instruction
  fault, and an expected CPL3 supervisor-page fault
* stateful monotonic physical-frame allocation
* a fixed 1 MiB supervisor-only, writable, NX kernel heap with unmapped
  guard pages
* a global first-fit bitmap allocator with alignment-aware reuse and
  coalescing of adjacent free allocation units
* Rust `alloc` support for `Box`, growing `Vec`, and `String`
* deterministic heap success, OOM, and guard-page QEMU tests
* bounded single-core preemptive round-robin scheduling for ring-0 kernel
  tasks, with generation-aware IDs and deterministic task lifecycle
* eight centrally reserved 16 KiB kernel task stacks, supervisor-only RW/NX,
  with unmapped guard pages above and below
* dedicated timer IRQ context switching with complete general-purpose
  register and interrupt-frame preservation; process-owned CPL3 frames are
  preempted while legacy bootstrap CPL3 tests keep their boundary-test path
* deterministic QEMU scheduler, CPU-bound preemption, stack-guard, capacity,
  and stale-ID/reuse coverage
* generation-safe ProcessId handles, monotonic PIDs, and a bounded process
  lifecycle with Zombie/reap semantics
* audited per-process address spaces, inactive-root construction, CR3 switching,
  and same-VA process isolation
* dedicated user stacks and per-task supervisor kernel-entry stacks, with
  TSS.RSP0 updates and CPL3 timer preemption
* checked static ELF64 loading with W^X, zeroed BSS/pages, and an embedded
  userspace smoke executable on normal boot
* provisional `getpid`/`exit`, checked user-copy APIs, and process-level
  #PF/#GP/#UD containment
* deterministic QEMU process, isolation, capacity, ELF, user-copy, and fault
  containment scenarios
* structured serial diagnostics
* panic handler
* an interrupt-enabled idle loop (plus a separate fatal halt path)

Not implemented:

* APIC initialization
* keyboard or mouse input
* general physical-frame deallocation/reuse (heap and process backing frames
  remain kernel-lifetime owned)
* dynamic heap growth, per-process heaps, or a slab allocator
* dynamic task stack growth, task join, blocking, sleep, or priorities
* APIC, SMP, and multicore scheduling
* VFS, initrd, filesystem, `/bin/init`, or file descriptors
* fork, general exec/spawn syscall, dynamic linking, shared libraries, PIE, or
  relocations
* userspace thread API, signals, pipes, PTY, shared memory, or general IPC
* user heap, `brk`, `mmap`, demand paging, copy-on-write, swap, or general
  physical-frame reclamation
* APIC/SMP/multicore scheduling, storage, networking, users/groups,
  capabilities, or GUI/desktop
* device drivers beyond serial, framebuffer, PIC, and PIT

pvKernel's 0.0.9 process and syscall interfaces are experimental. It does not
claim production readiness, general hardware support, a stable userspace ABI,
or readiness as a general-purpose kernel.

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

## Run the optional post-unmap page-fault test

Maps a temporary page, writes and verifies a sentinel value, unmaps it, and
deliberately re-accesses it to exercise the page-fault handler's
not-present classification. Disabled by default:

```bash
cargo run --features mem-fault-tests
```

See [docs/development/memory-testing.md](docs/development/memory-testing.md)
for the expected output.

## Run the deterministic headless QEMU tests

Verifies PIC/PIT initialization, repeated timer interrupt delivery, and the
memory-mapping smoke test (and, with `fault-tests`/`mem-fault-tests` also
enabled, their respective expected page faults), then exits QEMU with a
pass/fail status instead of leaving a graphical window open:

```bash
cargo run --features qemu-test -- --headless-test
cargo run --features "qemu-test fault-tests" -- --headless-test
cargo run --features "qemu-test mem-fault-tests" -- --headless-test
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
    ├── physical frame allocator + active page table
    ├── kernel heap mappings + global allocator
    ├── PIC remap + PIT configuration
    ├── memory-mapping smoke test
    ├── scheduler and process tables
    ├── audited address spaces and userspace smoke ELF
    ├── framebuffer
    ├── controlled breakpoint test
    ├── timer interrupt verification and process/task preemption
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
* [docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md) —
  physical/virtual memory foundation
* [docs/development/build.md](docs/development/build.md) — build environment
* [docs/development/debugging.md](docs/development/debugging.md) —
  debugging and diagnostics
* [docs/development/testing.md](docs/development/testing.md) —
  deterministic headless QEMU tests
* [docs/development/memory-testing.md](docs/development/memory-testing.md) —
  memory subsystem boot-time tests
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
