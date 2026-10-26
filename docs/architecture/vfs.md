# Volume-aware VFS

pvKernel 0.0.10 uses a bounded volume-qualified namespace. It separates four
concepts:

* A **volume** is a named namespace such as `Boot:` or a future `C:`.
* A **filesystem provider** implements lookup and returns node metadata/data.
* A **path** selects a volume and its components.
* An **executable format** is interpreted by the process loader after VFS
  resolution; it is not a filesystem type or volume identity.

The VFS knows none of the CPIO format. It resolves a path to a volume, passes
validated components to that volume's provider, and exposes node kind and
bounded regular-file reads. The CPIO initrd is one provider implementation.

## Path grammar and normalization

The bounded grammar is:

```text
VolumeName:\\component\\component
```

Canonical serialization uses backslashes. `/` is accepted as a separator
alias for internal/developer convenience, then serialized as `\\`. Repeated
separators and trailing separators collapse. `.` is ignored. `..` removes one
component and is rejected if it would escape the selected volume root.
`Volume:` without a root separator and drive-relative forms such as
`Volume:relative` are invalid. UNC and DOS device paths are not supported.

The total input is bounded to 255 UTF-8 bytes, a volume name to 16 ASCII
characters, each component to 63 UTF-8 bytes, and a path to 124 components.
Volume names start with an ASCII letter and continue with ASCII letters,
digits, `_`, or `-`. Volume matching is ASCII case-insensitive. Components
remain UTF-8 and case-preserving. Parsing and normalization never lowercase,
uppercase, Unicode-fold, or otherwise change component case. NUL, control
bytes, and `:` inside components are rejected. These are pvOS kernel rules,
not Win32 path compatibility behavior.

Examples:

```text
Boot:\\
Boot:\\pvOS\\System\\Init
C:\\Programs\\Calculator.pvapp
D:\\Users\\Petr\\Documents
```

Parsing `C:` can succeed while resolution returns `VolumeNotFound`; only
`Boot:` is registered in 0.0.10.

## Volume registry and provider contract

The registry has a fixed capacity of eight volumes, rejects invalid names and
duplicate names (case-insensitively), and returns `VolumeTableFull` at
capacity. The 0.0.10 registry is assembled during boot and published once
before interrupts are enabled; it contains only `Boot:`. Providers are
immutable after publication. There is no VFS lock, no allocation during
lookup, and no filesystem work on PIT scheduling paths. No filesystem lock
crosses a scheduler handoff, CR3 switch, or CPL3 entry. A future mutable or
hot-plug registry must define IRQ-safe synchronization and lock order before
it is introduced.

`FilesystemProvider::lookup` receives only validated case-preserving
components, not raw caller strings or a volume name. The generic VFS does not
decide whether component matching is case-sensitive; that policy belongs to
the provider. The `Boot:` CPIO provider compares archive path bytes exactly,
so its lookup is case-sensitive. Future providers may define another explicit
policy. This separation is intentional for filesystem interoperability. It
does not add Unicode case folding or Windows/NTFS matching behavior.

The provider returns regular-file or directory metadata and a handle. `read`
provides bounded random access so a future disk filesystem need not
materialize a full file as one contiguous slice. An optional `contiguous_file`
view supports zero-copy access for memory-backed providers such as the initrd.
Lookup reports
`VolumeNotFound`, `NotFound`, or `NotDirectory` distinctly. Directory reads
return `IsDirectory`; offsets beyond EOF return `InvalidOffset`; reads at EOF
return zero bytes. The contiguous immutable byte view is useful for the
memory-backed initrd but is not required of future providers.

## Boot volume and process boundary

The bootloader-provided CPIO `newc` archive is registered as the temporary
`Boot:` volume. The first process is resolved at
`Boot:\\pvOS\\System\\Init`. `spawn_path` requires a regular file and passes
its bytes to `process::spawn_elf`; only the process subsystem handles ELF,
address spaces, guarded stacks, rollback, and atomic process/task publication.
Init remains parentless numeric PID 1 and enters at CPL3.

The VFS is read-only. It has no file descriptors, filesystem syscalls,
permissions, symlinks, disk devices, partition discovery, or persistent
volumes. It does not claim Windows compatibility.

## Future interoperability direction

The provider boundary is intended to permit multiple filesystem types without
teaching path lookup about their formats. Possible future configurations
include `Boot:` backed by CPIO, `C:` backed by pvFS or another system
filesystem, `D:` backed by NTFS, and removable volumes backed by exFAT or
FAT32. These are examples only; no mapping is permanent and none of those
drivers or assignment policies is implemented in 0.0.10.
