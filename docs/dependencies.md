# Dependency review

This document reviews pvKernel's direct dependencies as declared in
[Cargo.toml](../Cargo.toml) and [kernel/Cargo.toml](../kernel/Cargo.toml).
License identifiers below are as reported by `cargo metadata` against the
committed `Cargo.lock`.

## bootloader (root package, build-dependency)

* **Purpose**: builds a bootable UEFI disk image from the compiled `kernel`
  binary artifact (`bootloader::UefiBoot::create_disk_image`, used in
  [build.rs](../build.rs)).
* **Why external**: writing a spec-compliant UEFI bootloader (page table
  setup, boot services handling, `BootInfo` construction) is a large,
  security-sensitive undertaking outside pvKernel's current scope.
* **What pvKernel still owns**: everything from `kernel_main` onward,
  including the GDT/TSS, IDT, and framebuffer rendering.
* **no_std status**: used only in the root `pvkernel-runner` build script,
  which runs on the host and is not part of the kernel binary itself.
* **Replacement boundary**: replacing this dependency would mean writing and
  maintaining a custom UEFI bootloader, listed as out of scope for 0.0.2 and
  not currently roadmapped.
* **License**: MIT OR Apache-2.0, compatible with pvKernel's MIT OR
  Apache-2.0 license.
* **Trust boundary**: handles the on-disk boot image layout at build time;
  it does not process untrusted runtime input.

## bootloader_api (kernel dependency)

* **Purpose**: defines the `BootInfo`, `MemoryRegion`, and `FrameBuffer`
  types that the bootloader passes to `kernel_main`, and the `entry_point!`
  macro used to register it.
* **Why external**: it is the shared contract between the `bootloader` crate
  and the kernel; both sides must agree on its layout.
* **What pvKernel still owns**: interpreting and using the boot information
  (memory map summarization, framebuffer rendering) is entirely pvKernel
  code.
* **no_std status**: `no_std`-compatible; used directly in the kernel
  binary.
* **Replacement boundary**: pinned to the exact same version as the root
  `bootloader` crate (`=0.11.17`), since the two must stay in lockstep.
* **License**: MIT OR Apache-2.0.
* **Trust boundary**: the `BootInfo` it describes originates from the
  bootloader/firmware, which pvKernel currently trusts as part of its boot
  chain.

## x86_64 (kernel dependency)

* **Purpose**: provides safe or narrowly-scoped-unsafe wrappers over x86_64
  CPU structures and instructions: `GlobalDescriptorTable`,
  `TaskStateSegment`, `InterruptDescriptorTable`, segment register access,
  `Cr2`, `hlt`/`int3`, and port I/O (`Port`/`PortWriteOnly`) used by the
  legacy PIC and PIT drivers.
* **Why external**: reimplementing these primitives would duplicate a
  well-reviewed, widely used crate without adding value, and would expand
  pvKernel's unsafe surface area for no benefit.
* **What pvKernel still owns**: how these primitives are assembled — the
  actual GDT/TSS/IDT contents, the double-fault IST stack, all exception and
  IRQ handler logic, and the PIC/PIT drivers, in `kernel/src/arch/x86_64/`.
* **no_std status**: `no_std`-compatible; this is a foundational crate for
  bare-metal x86_64 development.
* **Replacement boundary**: pvKernel uses its existing APIs rather than
  reimplementing CPU instructions that are already safely exposed by it, per
  project policy.
* **License**: MIT/Apache-2.0.
* **Trust boundary**: operates entirely on kernel-controlled CPU state; does
  not process external/untrusted data.

## uart_16550 (kernel dependency)

* **Purpose**: drives the 16550 UART for COM1 serial diagnostics, wrapped by
  `kernel/src/drivers/serial.rs`.
* **Why external**: a well-scoped, minimal driver for a well-understood
  legacy device; reimplementing it would not reduce the trusted computing
  base meaningfully.
* **What pvKernel still owns**: the `kprint!`/`kprintln!` macros, lock
  discipline (via `spin::Mutex` and `spin::Once`), and all log formatting.
* **no_std status**: `no_std`-compatible.
* **Replacement boundary**: could be replaced by a hand-written UART driver
  if 16550 support ever became insufficient; not currently justified.
* **License**: MIT.
* **Trust boundary**: writes only kernel-generated diagnostic text; does not
  parse untrusted input.

## spin (kernel dependency)

* **Purpose**: provides `spin::Once` and `spin::Mutex`, used for the
  kernel-owned GDT/TSS/IDT static storage and for serializing serial writes,
  without requiring `std` or a kernel heap.
* **Why external**: implementing correct, no_std-safe synchronization
  primitives from scratch is easy to get subtly wrong (memory ordering,
  reentrancy); `spin` is a small, widely used crate for exactly this need.
* **What pvKernel still owns**: deciding what state needs `Once`/`Mutex`
  protection and how fallback behavior works when a lock is already held
  (see `drivers::serial::_print`).
* **no_std status**: `no_std`-compatible by default.
* **Replacement boundary**: a small, clear local implementation was
  considered, but `spin` already provides exactly what is required for the
  descriptor tables' stable static addresses without adding meaningful
  unsafe surface.
* **License**: MIT.
* **Trust boundary**: operates entirely on kernel-internal state.

## ovmf-prebuilt (root package dependency)

* **Purpose**: fetches and caches prebuilt OVMF UEFI firmware images used to
  boot the kernel in QEMU (`src/main.rs`, the root runner).
* **Why external**: building OVMF from source is a large, separate project
  (EDK II); using prebuilt firmware is standard practice for QEMU-based
  development.
* **What pvKernel still owns**: nothing about firmware behavior; this is
  purely a development/runner convenience.
* **no_std status**: not applicable; used only by the host-side runner
  binary, never linked into the kernel.
* **Replacement boundary**: a developer could supply their own OVMF build
  instead; not currently needed.
* **License**: MIT OR Apache-2.0.
* **Trust boundary**: downloads firmware over the network on first run and
  caches it locally; this is a development-time trust decision, not a
  runtime kernel concern.

## Note on security auditing

None of the dependencies listed above are claimed to have undergone a formal
security audit. No such audit records exist in this repository at this time.
