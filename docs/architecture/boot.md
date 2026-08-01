# Boot flow

This document describes how pvKernel currently boots, from firmware power-on
to the interrupt-enabled idle loop. The bootloader described here is the
external [rust-osdev/bootloader](https://github.com/rust-osdev/bootloader)
crate; it is not part of pvKernel.

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
9. pvKernel installs its own IDT (`kernel/src/arch/x86_64/interrupts.rs`),
   including handlers for all 16 remapped IRQ vectors (32-47).
10. pvKernel remaps the legacy 8259 PIC to vectors 32-47 and masks every IRQ
    line (`kernel/src/arch/x86_64/pic.rs`).
11. pvKernel configures PIT channel 0 as a periodic timer at approximately
    100 Hz (`kernel/src/arch/x86_64/pit.rs`).
12. pvKernel initializes framebuffer output and renders the centered blue
    `P` logo (`kernel/src/graphics/framebuffer.rs`).
13. pvKernel verifies breakpoint handling with a controlled `int3` call and
    confirms execution resumes afterward.
14. pvKernel unmasks PIT IRQ0, enables maskable hardware interrupts, and
    confirms the tick counter advances by at least 5 ticks (see
    [docs/architecture/timer.md](timer.md)).
15. If the `fault-tests` Cargo feature is enabled, pvKernel triggers a
    deliberate page fault and matches it against its expected faulting
    address.
16. pvKernel enters an interrupt-enabled idle loop
    (`kernel/src/halt.rs::idle_loop`), unless the `qemu-test` Cargo feature
    is enabled, in which case it exits QEMU deterministically instead (see
    [docs/development/testing.md](../development/testing.md)).

## Notes

- The rust-osdev bootloader's own page table mapping is still active when
  `kernel_main` runs; pvKernel does not currently replace it. Virtual memory
  management is a future milestone (see [ROADMAP.md](../../ROADMAP.md)).
- Steps 10-11 (PIC/PIT setup) happen before the framebuffer is initialized,
  but every IRQ line stays masked and maskable interrupts stay disabled
  until step 14, so no hardware interrupt can be delivered before the IDT,
  PIC, and PIT are all fully configured.
- The boot flow above matches `cargo run`. `cargo run --features
  fault-tests` performs one additional step after the timer verification: a
  deliberate page fault, described in
  [docs/development/debugging.md](../development/debugging.md).
- `cargo run --features qemu-test -- --headless-test` (optionally combined
  with `fault-tests`) follows the same steps but exits QEMU deterministically
  at the end instead of idling forever; see
  [docs/development/testing.md](../development/testing.md).
