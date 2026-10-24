# Deterministic QEMU tests

This document describes pvKernel's headless, deterministic QEMU tests: how
they exit with a pass/fail status instead of running (or hanging)
indefinitely, what they verify, and how to run them locally.

## Why a separate test mode

Normal `cargo run` boots into a graphical QEMU window and an
interrupt-enabled idle loop that never exits — useful for interactive
development, but unusable for CI, which needs a command that terminates on
its own with a pass/fail exit code. The `qemu-test` Cargo feature and the
root runner's `--headless-test` flag together provide that, without
changing the normal boot path when neither is used.

## The `qemu-test` Cargo feature

Enabling `qemu-test` (forwarded from the root `pvkernel-runner` package to
`kernel/qemu-test`) compiles in
[kernel/src/arch/x86_64/qemu.rs](../../kernel/src/arch/x86_64/qemu.rs),
which exposes `qemu::exit_success()` / `qemu::exit_failure()`. These write a
status byte to QEMU's `isa-debug-exit` I/O port (`0xF4`) and never return
(falling back to `halt::halt_forever` if QEMU somehow does not exit). With
`qemu-test` enabled:

* `kernel_main` calls `qemu::exit_success()` once task smoke, boot-time timer
  verification (see [docs/architecture/timer.md](../architecture/timer.md)),
  and any selected finite scheduler scenario complete, instead of entering
  the normal idle loop. Expected-fault scenarios exit from their matching
  handler after validating the fault.
* the panic handler calls `qemu::exit_failure()` instead of
  `halt::halt_forever()`.
* `interrupts::page_fault_handler` recognizes deliberately armed expected
  faults, including the kernel-task stack guard scenario, and calls
  `qemu::exit_success()` directly instead of treating a matching fault as
  fatal.

Without `qemu-test`, none of this code is compiled in at all — a plain
`cargo build`/`cargo run` never links against the isa-debug-exit port.

## The `--headless-test` runner flag

The root runner (`src/main.rs`) checks its own process arguments for
`--headless-test`. When present, it launches QEMU with:

```
-display none -device isa-debug-exit,iobase=0xf4,iosize=0x04
```

instead of the normal graphical `-vga std -display gtk,gl=off`. `-serial
stdio`, `-monitor none`, and `-no-reboot` are passed either way. This flag
is independent of the `qemu-test` Cargo feature: passing `--headless-test`
without building the kernel with `--features qemu-test` just adds a device
the kernel never writes to, so the run boots normally, headlessly, and
never exits on its own (it must be killed, for example by `timeout`).

## Exit status mapping

QEMU turns a write of value `v` to `isa-debug-exit` into its own process
exit status `(v << 1) | 1`. `qemu::ExitCode::Success` (`0x10`) therefore
becomes host status 33, and `Failure` (`0x11`) becomes 35. The runner, when
`--headless-test` is passed, only treats status 33 as a pass:

* exactly 33 → the runner exits with `ExitCode::SUCCESS` (process exit 0);
* any other status, or termination by signal (for example, a `timeout` kill)
  → the runner prints a diagnostic and exits with `ExitCode::FAILURE`.

Without `--headless-test`, the runner keeps its original behavior: any
normal (status 0) QEMU exit is treated as success.

## Running the tests locally

```bash
# Verifies PIC/PIT initialization and repeated timer interrupt delivery,
# then exits successfully.
timeout 60 cargo run --features qemu-test -- --headless-test

# Same as above, plus a deliberate page fault matched against its expected
# faulting address, exercised after the timer test.
timeout 60 cargo run --features "qemu-test fault-tests" -- --headless-test

# Two finite kernel tasks, guarded stacks, exit, and bootstrap resume.
timeout 60 cargo run --features "qemu-test scheduler-test" -- --headless-test

# Two CPU-bound tasks that can finish only after IRQ-driven preemption.
timeout 60 cargo run --features scheduler-preemption-test -- --headless-test

# Two non-yielding CPL3 processes must each observe the peer's shared test
# counter; the only path between them is PIT-driven scheduler preemption.
timeout 60 cargo run --features process-preemption-test -- --headless-test

# Embedded ELF checks getpid and must not return after exit; the kernel
# observes deferred Zombie publication and reaps the process.
timeout 60 cargo run --features process-smoke-test -- --headless-test

# Page-chunked user copies, including cross-page transfer and hostile-pointer
# rejection before any partial destination update.
timeout 60 cargo run --features user-copy-test -- --headless-test

# Expected task stack lower-guard page fault.
timeout 60 cargo run --features task-stack-guard-fault-test -- --headless-test

# Fixed task capacity, safe stack-slot reuse, and stale TaskId rejection.
timeout 60 cargo run --features "qemu-test task-capacity-test" -- --headless-test
```

Both commands should print the full boot log to standard output and exit
with status 0. Wrapping them in `timeout` is a defense against a
regression that stops the kernel from ever reaching its success exit (for
example, an IDT vector left unhandled, or a masked/unmasked IRQ line
mismatch causing the timer test to hang) — the timeout guarantees such a
regression fails the run instead of hanging CI indefinitely.

See [docs/development/memory-testing.md](memory-testing.md) for the
`mem-fault-tests` post-unmap page-fault test, and
[docs/development/userspace-testing.md](userspace-testing.md) for the three
0.0.6 userspace boundary tests (`userspace-syscall-test`,
`userspace-privilege-fault-test`, `userspace-supervisor-fault-test`), which
build on this same mechanism.

The timer IRQ context-switch boundary and kernel-task limitations are
documented in [docs/architecture/kernel-tasks.md](../architecture/kernel-tasks.md).

## CI integration

`.github/workflows/ci.yml`'s `qemu-tests` job installs `qemu-system-x86` and
runs the timer, fault, memory, and userspace tests described above on every
pull request and push to `main`, in addition to the `check` job's
formatting and build verification.
