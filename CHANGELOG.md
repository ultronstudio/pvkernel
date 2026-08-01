# Changelog

All notable changes to pvKernel are described in this file. The format is
loosely inspired by [Keep a Changelog](https://keepachangelog.com/), without
claiming strict compliance.

## [Unreleased]

## [0.0.2] - 2026-08-01

### Added

- Modular kernel source layout (`arch`, `drivers`, `graphics`, `halt`).
- A kernel-owned Global Descriptor Table (GDT).
- A kernel-owned Task State Segment (TSS).
- A dedicated Interrupt Stack Table stack for double faults.
- A kernel-owned Interrupt Descriptor Table (IDT).
- Exception handlers for breakpoint, invalid opcode, general protection
  fault, page fault (with CR2 diagnostics), and double fault.
- A controlled `int3` boot-time verification of the breakpoint handler.
- An optional page-fault test behind the `fault-tests` Cargo feature.
- GitHub Actions CI running `cargo fmt --all -- --check` and `cargo build`.
- Project-specific contribution, code of conduct, architecture, roadmap,
  dependency, build, and debugging documentation.

### Changed

- Serial logging moved into a dedicated driver module with `kprint!`/
  `kprintln!` macros.
- Kernel initialization order: interrupts disabled, then GDT/TSS, then IDT,
  then framebuffer, then the controlled breakpoint test.
- Added `spin` as a kernel dependency for static, no_std-safe descriptor
  table storage.

## [0.0.1]

### Added

- UEFI boot through the rust-osdev bootloader.
- x86_64 kernel entry point.
- Boot memory map access.
- Linear framebuffer access with a centered blue `P` logo.
- Serial port (COM1) logging.
- Panic handler.
- CPU halt loop.
