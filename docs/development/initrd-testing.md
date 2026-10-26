# Boot volume and PID 1 validation

The root build builds `userspace/init`, stages its ELF, creates a deterministic
CPIO `newc` archive with `pvOS/`, `pvOS/System/`, and `pvOS/System/Init`, then
passes it to `UefiBoot::set_ramdisk`. The bootloader-delivered archive is
registered as `Boot:` by the kernel. Developers do not need to create it
manually.

## Local checks

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo build --locked
cargo build --locked --features initrd-boot-test
```

Host tests cover bounded volume/path parsing, CPIO validation, provider
adaptation, volume registration/lookup, directory traversal, regular-file
reads, and EOF/offset behavior. The generated `pvkernel-initrd.cpio` is next
to the staged kernel and UEFI image in the root runner's Cargo `OUT_DIR`. The
release action runs `scripts/validate_initrd.py`, checks the canonical
`pvOS/System/Init` archive entry and static ELF policy, and includes the
validated archive beside the mandatory bootable image.

## QEMU success scenario

```powershell
cargo run --locked --features initrd-boot-test -- --headless-test
```

This scenario requires:

```text
UEFI → BootInfo ramdisk → checked CPIO → Boot: registration
     → Boot:\\pvOS\\System\\Init → checked ELF → PID 1 → CPL3 marker
```

The kernel accepts the marker only from current user PID 1, then exits QEMU
successfully through `isa-debug-exit`.

## Controlled rejection scenarios

```powershell
cargo run --locked --features initrd-missing-test -- --headless-test
cargo run --locked --features initrd-corrupt-test -- --headless-test
cargo run --locked --features initrd-missing-volume-test -- --headless-test
cargo run --locked --features initrd-volume-registration-test -- --headless-test
cargo run --locked --features initrd-missing-init-test -- --headless-test
cargo run --locked --features initrd-invalid-init-test -- --headless-test
cargo run --locked --features initrd-wx-init-test -- --headless-test
```

Each scenario succeeds only when the selected boot-fatal condition is
recognized. The registration case deliberately attempts a duplicate
`Boot:` registration; the missing-volume case validates a good archive but
omits volume registration. An unexpectedly accepted condition exits with
failure.
