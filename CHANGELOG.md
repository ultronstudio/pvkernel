# Changelog

All notable changes to pvKernel are described in this file. The format is
loosely inspired by [Keep a Changelog](https://keepachangelog.com/), without
claiming strict compliance.

## [Unreleased]

### Changed

* `BootInfo`/`MemoryRegions` ownership is now moved, not aliased: the
  physical frame allocator's memory map is taken out of `BootInfo` by
  value (`core::mem::replace`) instead of being reconstructed as a
  `'static` slice via `core::slice::from_raw_parts` while a separate
  mutable `BootInfo` reference stayed reachable. See
  [docs/architecture/virtual-memory.md](docs/architecture/virtual-memory.md).
* Paging-mode (CR4.LA57) validation now runs and panics loudly *before*
  the active page table is ever reconstructed, instead of being checked
  and merely logged afterward.
* The Rust nightly toolchain is now pinned to an exact date
  (`rust-toolchain.toml`) instead of floating on `channel = "nightly"`.
* The pvKernel version string (boot banner, QEMU window title) is now
  derived from `CARGO_PKG_VERSION`/workspace-inherited package metadata
  instead of being hardcoded in multiple places.
* CI (`.github/workflows/ci.yml`) was split into independently-visible
  jobs (format, clippy, build matrix, and one job per deterministic QEMU
  scenario) so a single failing scenario can't hide behind an
  aggregate job.

### Added

* `.github/workflows/security.yml`: CodeQL (Rust), `cargo audit`,
  `cargo deny`, GitHub Dependency Review, and `actionlint`, with SARIF
  results uploaded to GitHub Code Scanning where applicable.
* `.github/workflows/release-candidate.yml` and
  `.github/workflows/release.yml`: validated, artifact-producing release
  pipelines (checksums, SPDX SBOM, release manifest, GitHub artifact
  attestation), gated on the full correctness/security suite and, for
  stable releases, a protected `release` environment approval.
* `deny.toml`: dependency advisory/license/source policy for
  `cargo deny`.
* `docs/security/vulnerability-management.md`: the `PVK-YYYY-NNNN`
  internal vulnerability identifier scheme, finding lifecycle, and
  AI-assisted security policy.
* `docs/audits/2026-10-pvkernel-0.0.6-pre-release.md`: first hardening-pass
  audit record.

This is a process/correctness hardening pass; no kernel-visible
functionality changed and the package version stays `0.0.6`.

## [0.0.6] - Userspace Entry and Syscall Foundation

### Added

* Dedicated ring-3 (CPL3) code/data GDT descriptors
  (`kernel::arch::x86_64::gdt::user_code_selector` /
  `user_data_selector`) and a dedicated, writable TSS.RSP0
  ring-transition kernel stack (`gdt::syscall_stack_top`), verified
  writable at boot the same way the double-fault IST stack already is.
* The first CPL0 → CPL3 transition
  (`kernel::arch::x86_64::userspace::enter::enter_user_mode`), built by
  hand-constructing an `iretq` stack frame and zeroing every
  general-purpose register beforehand so no kernel value ever leaks into
  userspace.
* A minimal, provisional, scalar-only `int 0x80` syscall ABI
  (`kernel::arch::x86_64::syscall`): `RAX` holds the syscall number on
  entry and the result on return (bit 63 set marks an error, see
  `syscall::error::SyscallError`); arguments are passed in
  `RDI`/`RSI`/`RDX`/`R10`/`R8`/`R9`. Implemented via a naked-asm entry
  stub (`syscall::entry::pvkernel_syscall_entry_stub`) that builds a
  `SyscallFrame` on the RSP0 stack and calls a Rust-side dispatcher
  (`syscall::dispatch::pvkernel_syscall_dispatch`), rather than
  `SYSCALL`/`SYSRET`, which remain out of scope for this milestone.
* Two documented, always-compiled bootstrap syscalls
  (`syscall::numbers::ABI_VERSION_QUERY`, `DEBUG_WRITE_BYTE`) plus a
  `TIMER_TICKS` query, and a small set of test-only syscalls under the
  `userspace-test` feature.
* `Permissions::USER_DATA` and `Permissions::USER_CODE`
  (`kernel::arch::x86_64::memory::paging`), reusing the existing W^X
  mapping API - a user code page is populated writable+non-executable,
  then switched to read-only+executable, and is never both at once.
* Three hand-written assembly test payloads and matching deterministic,
  headless QEMU tests, each behind its own Cargo feature
  (`userspace-syscall-test`, `userspace-privilege-fault-test`,
  `userspace-supervisor-fault-test`, all implying the base
  `userspace-test` feature): a CPL3 syscall-boundary test (exercises an
  unknown syscall number, `ABI_VERSION_QUERY`, `DEBUG_WRITE_BYTE`,
  confirms a PIT tick can interrupt and resume CPL3 execution, then
  exits via a test-only `TEST_EXIT` syscall); a CPL3 privilege-instruction
  fault test (`cli` at CPL3 raises `#GP`, matched by the general
  protection fault handler); and a CPL3 supervisor-page fault test (a
  CPL3 read of a mapped-but-supervisor-only page raises `#PF`, matched by
  the page fault handler).
* CPL-origin diagnostics in the general-protection-fault and page-fault
  handlers (`kernel::arch::x86_64::interrupts`), logging whether the
  fault originated at CPL0 or CPL3.
* New documentation:
  [docs/architecture/userspace-entry.md](docs/architecture/userspace-entry.md),
  [docs/architecture/syscalls.md](docs/architecture/syscalls.md), and
  [docs/development/userspace-testing.md](docs/development/userspace-testing.md).
* CI coverage for all three new `userspace-*-test` features (build, and
  build+run under `qemu-test`).

### Changed

* The root runner's window title is now `pvKernel 0.0.6`.
* [ROADMAP.md](ROADMAP.md)'s 0.0.6 milestone is redefined as "Userspace
  Entry and Syscall Foundation" (previously "Kernel Heap and Rust `alloc`
  Support"); the kernel heap moves to 0.0.7, the scheduler to 0.0.8, and
  the standalone "Ring 3 and System Calls" milestone previously planned
  for 0.0.8 is folded into 0.0.9 alongside the ELF loader, since this
  release already provides the ring-3/syscall foundation. See
  [ROADMAP.md](ROADMAP.md#006--userspace-entry-and-syscall-foundation)
  for the full reordering rationale.

### Known limitations

* No `SYSCALL`/`SYSRET` - only the software-interrupt `int 0x80` path is
  implemented.
* No process model, no threads, no scheduler: userspace execution in this
  milestone exists solely to exercise the CPL0 → CPL3 transition and the
  bootstrap syscall ABI in a deterministic test, not to run general
  programs.
* No ELF loader - the three test payloads are hand-written machine code,
  not compiled or loaded from a binary format.
* No `copy_from_user`/`copy_to_user` or any user-pointer dereferencing by
  kernel code; every current syscall argument is treated as an opaque
  scalar.
* The supervisor-page-fault test's probe address is baked into the test
  payload as a raw immediate, kept in sync with the Rust-side constant
  only by a compile-time assertion, rather than patched at runtime.

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
* No kernel heap or Rust `alloc` support yet (now planned for 0.0.7; see
  the [0.0.6](#006---userspace-entry-and-syscall-foundation) entry above
  for the reordering).
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
