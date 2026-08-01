# Contributing to pvKernel

Thank you for your interest in contributing to pvKernel.

## 1. Project status

pvKernel is experimental and in early development. Module layout, internal
APIs, and boot behavior may change between releases without a deprecation
period.

## 2. Required environment

- Linux or WSL2.
- Rust nightly, selected automatically via [`rust-toolchain.toml`](rust-toolchain.toml).
- Cargo (installed with Rust via [rustup](https://rustup.rs/)).
- QEMU with `qemu-system-x86_64` available on `PATH`.
- System packages needed to run QEMU with a graphical display (see
  [docs/development/build.md](docs/development/build.md) for a tested
  package list).
- Optional: GDB, if you intend to attach a debugger (see
  [docs/development/debugging.md](docs/development/debugging.md) for the
  current, limited state of debugger integration).

## 3. Repository setup

```bash
git clone https://github.com/ultronstudio/pvkernel.git
cd pvkernel
rustup show           # installs the pinned toolchain, components, and targets
cargo build
cargo run
```

## 4. Branch naming

Use a short, descriptive branch name prefixed by its category:

- `feature/gdt-cleanup`
- `fix/page-fault-diagnostics`
- `docs/interrupt-architecture`

## 5. Issue and pull-request expectations

- One logical change per pull request.
- Link the relevant issue, if one exists.
- Explain the motivation for the change, not just what changed.
- Explain how the change was tested.
- Keep pull requests reasonably small and focused.
- Do not mix formatting-only changes with behavioral changes in the same
  pull request.

## 6. Required checks

Before opening a pull request, run:

```bash
cargo fmt --all -- --check
cargo build
```

Additionally:

- Run `cargo run` when your change affects boot behavior.
- Run `cargo run --features fault-tests` when your change affects exception
  handling code.

## 7. Unsafe-code policy

- Every `unsafe` block requires a `// SAFETY:` comment explaining which
  invariant makes the operation valid, who establishes that invariant, and
  how it remains valid for as long as required.
- Do not use `unsafe` merely to work around the borrow checker.
- Avoid expanding the trusted computing base; prefer safe APIs already
  exposed by existing dependencies (for example, the `x86_64` crate) over
  reimplementing raw CPU instructions.
- Document hardware invariants (register state, memory layout, alignment)
  that the surrounding safe code relies on.

## 8. Dependency policy

Before adding a new dependency, verify and state in the pull request:

- why the dependency is needed;
- that it supports `no_std` (pvKernel has no standard library);
- that its license is compatible with MIT OR Apache-2.0;
- its maintenance status and the presence of `unsafe` code in it;
- that it does not duplicate functionality already available in the
  workspace.

## 9. Documentation policy

- Update the relevant architecture document
  ([ARCHITECTURE.md](ARCHITECTURE.md), [docs/architecture/](docs/architecture/))
  when you change kernel architecture.
- Update [CHANGELOG.md](CHANGELOG.md) for user-visible changes.
- Keep current-status claims in the README and architecture docs accurate;
  do not describe unimplemented functionality as implemented.

## 10. Commit style

Prefer conventional, descriptive commit subjects, for example:

- `feat: add breakpoint exception handler`
- `fix: use dedicated double-fault IST stack`
- `refactor: move serial output into driver module`
- `docs: describe x86_64 exception initialization`

## 11. AI-assisted contribution policy

AI-assisted development is permitted, but contributors remain fully
responsible for understanding, reviewing, testing, and documenting every
submitted change.

Unexplained generated code, unverified unsafe code, fabricated test results,
or pull requests that the contributor cannot explain will not be accepted.

Contributors are not required to disclose the specific editor or assistance
tool they used, unless legally required.

## 12. Licensing

Contributions are accepted under MIT OR Apache-2.0, matching the project's
existing license.

## 13. Security reports

Do not report suspected security vulnerabilities in public issues or pull
requests. See [SECURITY.md](SECURITY.md) for the private reporting process.
