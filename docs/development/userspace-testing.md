# Userspace boundary tests

This document describes pvKernel 0.0.6's three deterministic, headless QEMU
tests for the CPL0 → CPL3 transition and the bootstrap syscall ABI. See
[docs/development/testing.md](testing.md) for the general headless-QEMU test
mechanism (`qemu-test` feature, `--headless-test` flag, exit-status mapping)
these tests build on, and
[docs/architecture/userspace-entry.md](../architecture/userspace-entry.md) /
[docs/architecture/syscalls.md](../architecture/syscalls.md) for the
subsystems they exercise.

## Cargo features

All three tests are disabled by default. Each is selected by its own Cargo
feature, all of which imply the base `userspace-test` feature:

* `userspace-syscall-test`
* `userspace-privilege-fault-test`
* `userspace-supervisor-fault-test`

Exactly one must be enabled at a time — enabling more than one, or enabling
`userspace-test` directly without exactly one of the three, is a compile
error (`compile_error!` guards in `userspace::mod`), not a silently
ambiguous runtime choice.

## Syscall-boundary test (`userspace-syscall-test`)

Maps a hand-written payload into the reserved userspace range and enters it
at CPL3. The payload:

1. confirms it is actually executing at CPL3 (checks `CS` RPL);
2. sets six sentinel register values (`RBX`, `RBP`, `R12`-`R15`);
3. issues an unknown syscall number (`0xdead`) and confirms it returns the
   `Unsupported` error;
4. issues `ABI_VERSION_QUERY` and confirms it returns `1`;
5. issues `DEBUG_WRITE_BYTE(0x42)` and confirms it returns `0`;
6. re-checks all six sentinel registers are unchanged (the entry stub must
   preserve every register it did not itself use as an argument/result);
7. polls the test-only `TEST_STATUS` syscall in a bounded loop, waiting for
   the bit that means "a PIT tick was observed while executing at CPL3" —
   confirming hardware interrupts can preempt and correctly resume CPL3
   execution;
8. calls the test-only `TEST_EXIT` syscall with `RDI = 0` (success), or
   jumps to a failure path calling it with `RDI = 1` if any check above
   failed.

Run it with:

```bash
timeout 60 cargo run --features "qemu-test userspace-syscall-test" -- --headless-test
```

Expected final lines:

```
[userspace] preparing syscall-boundary test payload
[userspace] user code page mapped RX+U at 0x600000000000, stack RW+NX+U at 0x600000002000
[userspace] entering CPL3 at 0x600000000000
[syscall] debug byte from CPL3: 0x42
[syscall] test_exit(success) from CPL3 userspace payload
```

## Privilege-fault test (`userspace-privilege-fault-test`)

Maps a payload that executes `cli` at CPL3. Since `enter_user_mode` always
sets IOPL 0, `cli` is an IOPL-sensitive instruction the CPU rejects at
CPL3, raising `#GP(0)`. Before entering CPL3, the test arms an expected-fault
flag (`interrupts::arm_expected_privilege_fault`); the general-protection-fault
handler checks the fault originated at CPL3 with this flag armed and exits
QEMU successfully instead of treating it as fatal.

Run it with:

```bash
timeout 60 cargo run --features "qemu-test userspace-privilege-fault-test" -- --headless-test
```

Expected final lines:

```
[userspace] preparing privilege-fault test payload
[userspace] entering CPL3 at 0x600000000000
[exception] general protection fault
origin: CPL3
error code: 0x0
... (interrupt stack frame) ...
[test] expected CPL3 privilege fault matched
```

## Supervisor-page-fault test (`userspace-supervisor-fault-test`)

Maps a fixed probe page as supervisor-only (`Permissions::KERNEL_DATA`, not
user-accessible), then maps a payload that reads from it at CPL3. This
raises `#PF` with the `PROTECTION_VIOLATION` and `USER_MODE` bits set (and
`CAUSED_BY_WRITE`/`INSTRUCTION_FETCH` clear — a plain CPL3 read). Before
entering CPL3, the test arms an expected-fault check
(`interrupts::arm_expected_supervisor_fault`) with the probe address; the
page-fault handler matches the faulting address and error-code bits and
exits QEMU successfully instead of treating it as fatal.

The probe address is baked into the payload as a raw assembly immediate
(`0xffff900000002000`), kept in sync with the Rust-side constant only by a
compile-time assertion (`const _: () = assert!(...)`) — a deliberate
simplification rather than patching the address into the payload at
runtime.

Run it with:

```bash
timeout 60 cargo run --features "qemu-test userspace-supervisor-fault-test" -- --headless-test
```

Expected final lines:

```
[userspace] preparing supervisor-page-fault test payload
[userspace] entering CPL3 at 0x600000000000
[exception] page fault
faulting address (CR2): 0xffff900000002000
origin: CPL3
error code: PageFaultErrorCode(PROTECTION_VIOLATION | USER_MODE)
... (interrupt stack frame) ...
[test] expected CPL3 supervisor-page fault matched
```

## Behavior without `qemu-test`

Without the `qemu-test` feature, each expected-fault "arm" function is a
no-op, and a matching fault falls through to the same fatal-halt path an
unexpected fault would — the kernel logs the fault and halts (or panics),
matching the existing `fault-tests`/`mem-fault-tests` convention. This is
expected behavior for a manually observed run, not a hang: a human running
one of these builds interactively can still visually confirm the right kind
of fault occurred from the serial log, even without an automated pass/fail
signal.

## Mutual exclusivity with other test features

These three features are only meaningfully combined with `qemu-test` (for
a deterministic exit) — they are independent of, and can be built alongside,
`fault-tests`/`mem-fault-tests`, though the three userspace scenarios are
mutually exclusive with each other (see above), since `userspace::run_test`
dispatches to exactly one scenario and never returns.
