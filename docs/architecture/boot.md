# Boot flow

This document describes pvKernel 0.0.10 from UEFI firmware to PID 1. The
rust-osdev bootloader is pinned to `0.11.17`.

## Image construction

The root Cargo build separately compiles the kernel and `userspace/init` for
`x86_64-unknown-none`. `build.rs` creates a deterministic CPIO `newc` archive
with `pvOS/`, `pvOS/System/`, and `pvOS/System/Init`, then supplies it through
the pinned `bootloader::UefiBoot::set_ramdisk` API before creating the UEFI
disk image. Local and release builds use this same construction path.

## Runtime sequence

1. UEFI starts the bootloader from the generated disk image.
2. Bootloader `0.11.17` loads the kernel and ramdisk independently. `BootInfo`
   reports the ramdisk's mapped virtual address and byte length.
3. `kernel_main` saves framebuffer and ramdisk metadata, then transfers
   `BootInfo` to architecture initialization.
4. pvKernel installs GDT/TSS and IDT, initializes memory/paging, audits and
   retains boot mappings including the ramdisk, then configures PIC and PIT.
5. Heap, scheduler, and process subsystems initialize. After the mapping
   audit, the kernel validates ramdisk metadata and creates an immutable view
   without copying the archive.
6. The checked CPIO parser validates the complete archive. `InitrdFs` adapts
   its entries to the format-independent filesystem-provider interface.
7. The VFS registers the provider under the temporary `Boot:` volume name.
   No persistent `C:` volume is discovered or implied.
8. Normal boot resolves `Boot:\\pvOS\\System\\Init`, requires a regular
   file, and passes its archive-backed bytes to `process::spawn_elf`.
9. `spawn_elf` reserves a parentless process, builds its isolated address
   space and guarded user stack, applies the existing ELF64 and W^X policy,
   prepares its CPL3 task, and atomically publishes the process/task pair.
   The initial process-table PID is 1.
10. Once PIT IRQ0 is unmasked and interrupts are enabled, the scheduler enters
    init at CPL3. It checks `getpid() == 1`, emits the `I` diagnostic marker,
    and remains alive in a scalar-safe loop.

Missing ramdisk metadata, malformed CPIO, volume registration/lookup failure,
missing or non-regular boot init, ELF rejection, W^X rejection, process
construction failure, or a PID 1 invariant failure is fatal in normal boot.
There is no embedded executable fallback and no `/bin/init` alias. QEMU test
builds route expected fatal cases through `isa-debug-exit`.

`Boot:` is temporary boot storage. A future boot may use it to load storage
drivers and then discover a persistent system volume. Volume discovery,
storage, and filesystem transition are outside 0.0.10.

See [initrd.md](initrd.md), [vfs.md](vfs.md), and
[../development/initrd-testing.md](../development/initrd-testing.md) for
transport, path rules, and test commands.
