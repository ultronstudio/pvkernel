# Changelog

All notable changes to pvKernel are described in this file. The format is
loosely inspired by [Keep a Changelog](https://keepachangelog.com/), without
claiming strict compliance.

## [Unreleased]

## [0.0.5] - Virtual Memory Foundation

### Added

* A monotonic boot-time physical frame allocator
  (`kernel::arch::x86_64::memory::frame_allocator::BootFrameAllocator`),
  built from the bootloader-provided memory map.
* Safe, narrowly-scoped access to the single active x86_64 page table
  (`kernel::arch::x86_64::memory::address_space::KernelAddressSpace`),
  reachable only through `memory::with_kernel_address_space`, which holds a
  `spin::Mutex` with maskable hardware interrupts disabled.
* 4 KiB map / unmap / permission-change / translate operations
  (`kernel::arch::x86_64::memory::paging`), with a project-specific
  `MemoryError` type covering non-canonical addresses, misalignment,
  already-mapped/not-mapped pages, frame-allocation failure, parent
  huge-page conflicts, invalid permission combinations, and invalid frame
  addresses.
* A W^X permission model (`Permissions`) enforced by the mapping API
  itself: requesting a writable-and-executable mapping is rejected before
  any page-table entry is touched.
* Runtime verification (not just assumption) of the current paging mode
  (4-level vs. 5-level) and whether EFER.NXE is active.
* A custom `BootloaderConfig` in `kernel/src/main.rs` requesting a
  physical-memory offset mapping (`mappings.physical_memory =
  Some(Mapping::Dynamic)`), required for page-table access; this was not
  previously requested, so `BootInfo::physical_memory_offset` was
  previously always `None`.
* A boot-time mapping smoke test (`memory::run_boot_smoke_test`), run
  unconditionally on every boot: rejects a non-canonical address, maps a
  temporary page, verifies its translation and flags, writes and reads
  back a sentinel value, changes it to read-only, unmaps it, and confirms
  it translates as unmapped again.
* An optional deterministic post-unmap page-fault test
  (`memory::run_unmap_fault_test`, behind the new `mem-fault-tests` Cargo
  feature), mirroring the existing `fault-tests` page-fault test: maps,
  writes, unmaps, then deliberately re-accesses a page, and (under
  `qemu-test`) exits QEMU successfully once the resulting fault is matched.
* New documentation:
  [docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md)
  and
  [docs/development/memory-testing.md](docs/development/memory-testing.md).
* CI coverage for the `mem-fault-tests` feature (build) and the combined
  `qemu-test mem-fault-tests` headless test (build and run).

### Changed

* `arch::x86_64::init` now takes `&mut BootInfo` and initializes the memory
  subsystem between the IDT and the PIC, so a page fault during memory
  initialization is still handled, and no other subsystem can create
  page-table mappings first.
* The root runner's window title is now `pvKernel 0.0.5`.
* [ROADMAP.md](ROADMAP.md)'s 0.0.5 milestone is redefined as "Virtual
  Memory Foundation" (previously "Physical Memory Allocator"), combining
  frame allocation and page-table access into a single milestone based on
  implementation experience; 0.0.6 is redefined as "Kernel Heap and Rust
  `alloc` Support" (previously "Virtual Memory and Kernel Heap", since
  page-table access moved to 0.0.5).

### Known limitations (see
[docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md#future-work-and-known-limitations)
for detail)

* The physical frame allocator is monotonic only: frames returned by
  `unmap_page` are not returned to any allocator and cannot be reused.
* No kernel heap or Rust `alloc` support yet (planned for 0.0.6).
* No guard pages.
* No dedicated write-protection (CR0.WP) boot-time test, even though
  read-only mappings are already supported and exercised as part of the
  smoke test.
* No SMP-aware locking beyond disabling interrupts plus a `spin::Mutex`
  (correct for pvKernel's current single-CPU operation only).

## [0.0.4] - 2026-08-01

### Added

* A legacy dual 8259 PIC driver (`kernel::arch::x86_64::pic`): IRQ vector
  remapping to 32-47, per-line masking/unmasking, end-of-interrupt handling
  (including the slave-then-master cascade order), and spurious IRQ7/IRQ15
  detection via the in-service register.
* A PIT channel 0 periodic timer driver (`kernel::arch::x86_64::pit`),
  configured for approximately 100 Hz, with an atomic tick counter and an
  uptime helper.
* IDT handlers for all 16 remapped IRQ vectors (32-47): a timer handler, two
  spurious-IRQ handlers, and generic handlers for every other line that
  pvKernel keeps masked in this release.
* A boot-time verification that unmasks the timer IRQ, enables maskable
  hardware interrupts, and confirms the tick counter advances before
  continuing.
* A distinction between fatal halting (`halt::halt_forever`, used for panics
  and fatal CPU exceptions) and an interrupt-enabled idle loop
  (`halt::idle_loop`), which normal boot now ends in instead of halting
  immediately.
* QEMU `isa-debug-exit` support (`kernel::arch::x86_64::qemu`, behind the new
  `qemu-test` Cargo feature) and a root runner `--headless-test` flag, so the
  timer and expected-page-fault tests can run deterministically and exit
  with a pass/fail status suitable for CI.
* A `qemu-tests` CI job running both headless tests on every pull request
  and push to `main`.
* New documentation:
  [docs/architecture/hardware-interrupts.md](docs/architecture/hardware-interrupts.md),
  [docs/architecture/timer.md](docs/architecture/timer.md), and
  [docs/development/testing.md](docs/development/testing.md).

### Changed

* The expected-page-fault test (`fault-tests` feature) now runs after the
  timer verification instead of before it, so a `fault-tests` build always
  exercises PIC/PIT timer interrupts before its deliberate fault.
* The root runner's window title is now `pvKernel 0.0.4`.
* The `qemu-tests` CI job now builds each headless test binary in an
  untimed step before running it, and raises the run timeout to 120
  seconds, since GitHub-hosted runners have no `/dev/kvm` and fall back to
  slower TCG emulation.

## [0.0.3] - CPU Exception Correctness

### Fixed

* The double-fault IST stack's backing storage is now explicitly writable
  (`UnsafeCell`-backed), instead of a plain immutable `static` that had no
  defined contract for the CPU's own writes into it.
* Documentation on `DOUBLE_FAULT_IST_INDEX` now distinguishes the zero-based
  Rust array index from the hardware IST slot encoding (1-7) applied by the
  `x86_64` crate's `set_stack_index`.
* Confirmed the CI toolchain declaration (`rust-toolchain.toml`) already
  installs the `rust-src` component required by the `bootloader` crate's
  `-Z build-std` step, so both local development and CI use the same
  declared toolchain.
* Translated the remaining Czech inline comments in
  `kernel/src/graphics/framebuffer.rs` to English, matching the rest of the
  repository's documentation language.

## [0.0.2] - 2026-08-01

### Added

* Modular kernel source layout (`arch`, `drivers`, `graphics`, `halt`).
* A kernel-owned Global Descriptor Table (GDT).
* A kernel-owned Task State Segment (TSS).
* A dedicated Interrupt Stack Table stack for double faults.
* A kernel-owned Interrupt Descriptor Table (IDT).
* Exception handlers for breakpoint, invalid opcode, general protection
  fault, page fault (with CR2 diagnostics), and double fault.
* A controlled `int3` boot-time verification of the breakpoint handler.
* An optional page-fault test behind the `fault-tests` Cargo feature.
* GitHub Actions CI running `cargo fmt --all -- --check` and `cargo build`.
* Project-specific contribution, code of conduct, architecture, roadmap,
  dependency, build, and debugging documentation.

### Changed

* Serial logging moved into a dedicated driver module with `kprint!`/
  `kprintln!` macros.
* Kernel initialization order: interrupts disabled, then GDT/TSS, then IDT,
  then framebuffer, then the controlled breakpoint test.
* Added `spin` as a kernel dependency for static, no_std-safe descriptor
  table storage.

## [0.0.1]

### Added

* UEFI boot through the rust-osdev bootloader.
* x86_64 kernel entry point.
* Boot memory map access.
* Linear framebuffer access with a centered blue `P` logo.
* Serial port (COM1) logging.
* Panic handler.
* CPU halt loop.
