# Build environment

Tested on Ubuntu 22.04/24.04 and WSL2 with an Ubuntu distribution.

> [!WARNING]
> Do not build or run this repository as `root`. Use a regular user account;
> QEMU's graphical display and Rust toolchain installation are designed for
> normal user sessions.

## 1. System packages

```bash
sudo apt update
sudo apt install -y qemu-system-x86 build-essential curl
```

`qemu-system-x86` provides `qemu-system-x86_64`, which the root runner
invokes directly.

## 2. Install Rust via rustup

If you do not already have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Restart your shell (or `source "$HOME/.cargo/env"`) so `cargo` and `rustup`
are on `PATH`.

## 3. Toolchain setup

pvKernel pins its toolchain in [rust-toolchain.toml](../../rust-toolchain.toml)
(a nightly channel with the `rustfmt`, `clippy`, and `llvm-tools` components,
and the `x86_64-unknown-none` and `x86_64-unknown-uefi` targets). From the
repository root:

```bash
rustup show
```

This installs the pinned toolchain, components, and targets if they are not
already present. You do not need to run `rustup default` or otherwise change
your global toolchain; the override is scoped to this repository via
`rust-toolchain.toml`.

## 4. Building

```bash
cargo build
```

This builds the `kernel` crate for the `x86_64-unknown-none` target as a
Cargo artifact dependency (see [docs/dependencies.md](../dependencies.md) for
why `bindeps` is enabled in [.cargo/config.toml](../../.cargo/config.toml)),
then builds the root `pvkernel-runner` binary and produces a UEFI disk image
via `build.rs`.

## 5. Running

```bash
cargo run
```

This downloads OVMF firmware on first run (cached under `target/ovmf`) and
launches `qemu-system-x86_64` with the produced UEFI image, COM1 attached to
standard output, and a graphical framebuffer window.

## 6. Running the optional fault test

```bash
cargo run --features fault-tests
```

See [docs/development/debugging.md](debugging.md) for the expected output
and how the page-fault handler behaves.

## Common build failures

- **`error: toolchain 'nightly-...' is not installed`**: run `rustup show`
  from the repository root to install the pinned toolchain.
- **`qemu-system-x86_64: command not found`**: install `qemu-system-x86`
  (see step 1).
- **QEMU window does not appear under WSL2**: see the WSLg notes in
  [docs/development/debugging.md](debugging.md).
- **`CARGO_BIN_FILE_KERNEL_kernel` not set / bindeps errors**: ensure
  [.cargo/config.toml](../../.cargo/config.toml) is present and unmodified;
  it enables the unstable `bindeps` feature required to depend on the
  `kernel` binary artifact from the root package's `build.rs`.
