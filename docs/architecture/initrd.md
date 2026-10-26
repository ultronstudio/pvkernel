# Boot volume transport and CPIO format

The UEFI image contains two independent payloads: the kernel ELF and the
`pvkernel-initrd.cpio` archive. The root `build.rs` builds the separately
targeted `pvkernel-init` artifact, writes a deterministic CPIO `newc` archive,
then passes that archive to the pinned `bootloader::UefiBoot::set_ramdisk`
API (bootloader `0.11.17`). The same build script creates the local and release
UEFI images.

The archive backs the temporary `Boot:` volume. It is not described as the
permanent pvOS system volume, and it does not establish a Unix `/` namespace.

## BootInfo ownership and mapping

Bootloader API `0.11.17` exposes `ramdisk_addr: Optional<u64>` and
`ramdisk_len: u64`. The UEFI implementation passes the address of its mapped
ramdisk slice, not its physical address. The bootloader maps it read/write and
NX in the dynamic mapping arena and marks the backing frames unavailable in
the memory map. pvKernel records the range in the boot mapping inventory;
the address-space audit checks it and retains the mapping in the kernel
address space. Only after that audit does `BootRamdisk::from_mapping` check
presence, nonzero length, checked end arithmetic, `usize`/`isize` bounds,
canonical endpoints, and containment within the audited dynamic arena, then
create a shared immutable slice. No archive copy or mutable Rust reference is
kept. The bootloader mapping remains supervisor read/write and NX, so
immutability is a kernel software invariant rather than a read-only page-table
permission. Current archive/VFS consumers only read the bytes, and the kernel
never writes to the ramdisk. The mapping and physical frames are kernel-
lifetime resources.

The unsafe slice construction relies on the pinned bootloader contract that
`ramdisk_addr` points to `ramdisk_len` mapped bytes, plus pvKernel's successful
mapping audit. If either contract changes, this conversion must be reviewed.

## Supported `newc` subset

The parser accepts magic `070701` and explicit 110-byte headers only. It
decodes hexadecimal fields byte by byte, checks every addition and slice
range, validates NUL-terminated UTF-8 names, verifies zero padding to 4-byte
boundaries, and bounds each file payload. Maximum path length is 255 bytes,
component length is 63 bytes, and the archive may contain at most 128
non-trailer entries. The archive must end with an empty `TRAILER!!!` entry;
only zero bytes may follow it.

Only regular files with link count 1 and empty directories are accepted.
Symlinks, devices, hard-link regular files, checksum-bearing headers, and
other node types are rejected. Paths in
the archive are relative canonical names using CPIO's slash separators:
`pvOS`, `pvOS/System`, and `pvOS/System/Init`. Absolute names, empty
components, `.`, and `..` are rejected. Duplicate names are an archive error.
File bytes and names remain borrowed from the immutable
ramdisk; entry lookup scans the validated archive and allocates no tree.

The separately built `pvkernel-init` ELF is reached through
`Boot:\\pvOS\\System\\Init` and linked as static `ET_EXEC` at the existing
loader's `0x0040_0000` user range. ELF64 is the internal bootstrap executable
representation only; it does not define the final pvOS application format.
The `_start` assembly trampoline aligns the initial 16-byte user stack before
calling the Rust init body; the kernel does not enable or save
floating-point/SIMD state for it.

Future pvOS may use `.pvx` for a native executable, `.pvapp` for an
application bundle, and `.pvpkg` for an installable package. For example, a
future `Calculator.pvapp` could contain `App.pvx`, a manifest, resources,
libraries, and signatures. Those formats, loaders, package management,
signing, and dynamic linking are not part of 0.0.10. A future `.pvx` may wrap
or contain ELF, but no container format is defined here.

The volume provider boundary also leaves room for future pvFS, NTFS, exFAT,
and FAT32 implementations. No persistent volume or disk filesystem exists
yet; persistent storage and the final executable/application ABI remain
future work for 0.1.x or later.

Malformed or absent initrd metadata and malformed archives are fatal for a
normal boot. There is no embedded executable fallback.
