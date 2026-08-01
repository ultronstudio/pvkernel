# Boot flow

This document describes how pvKernel currently boots, from firmware power-on
to the CPU halt loop. The bootloader described here is the external
[rust-osdev/bootloader](https://github.com/rust-osdev/bootloader) crate; it is
not part of pvKernel.

## Steps

1. QEMU starts OVMF (the UEFI firmware used in this project's runner).
2. OVMF loads the boot image produced by `build.rs`
   (`bootloader::UefiBoot::create_disk_image`).
3. The rust-osdev bootloader, embedded in that image, loads the pvKernel ELF
   binary.
4. The bootloader exits UEFI boot services, builds its own initial page table
   mapping and stack, and constructs a `bootloader_api::BootInfo` value
   describing the memory map and framebuffer.
5. Control transfers to `kernel_main` (registered via `entry_point!` in
   [kernel/src/main.rs](../../kernel/src/main.rs)).
6. pvKernel initializes serial output on COM1
   (`kernel/src/drivers/serial.rs`) and prints the boot header.
7. pvKernel disables maskable hardware interrupts
   (`kernel/src/arch/x86_64/mod.rs`).
8. pvKernel installs its own GDT and TSS, including the dedicated
   double-fault IST stack (`kernel/src/arch/x86_64/gdt.rs`).
9. pvKernel installs its own IDT (`kernel/src/arch/x86_64/interrupts.rs`).
10. pvKernel initializes framebuffer output and renders the centered blue
    `P` logo (`kernel/src/graphics/framebuffer.rs`).
11. pvKernel verifies breakpoint handling with a controlled `int3` call and
    confirms execution resumes afterward.
12. pvKernel halts the CPU (`kernel/src/halt.rs`).

## Notes

- The rust-osdev bootloader's own page table mapping is still active when
  `kernel_main` runs; pvKernel does not currently replace it. Virtual memory
  management is a future milestone (see [ROADMAP.md](../../ROADMAP.md)).
- The boot flow above matches `cargo run`. `cargo run --features
  fault-tests` performs one additional step after the breakpoint test: a
  deliberate page fault, described in
  [docs/development/debugging.md](../development/debugging.md).
