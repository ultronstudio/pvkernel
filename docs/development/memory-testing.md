# Memory subsystem tests

This document describes memory-subsystem boot-time tests added across pvKernel
0.0.5 and 0.0.7, including the unconditional mapping and heap smoke tests and
the optional deterministic post-unmap page-fault test. See
[docs/architecture/virtual-memory.md](../architecture/virtual-memory.md) for
the subsystem these tests exercise, and
[docs/development/testing.md](testing.md) for the general headless-QEMU test
mechanism (`qemu-test` feature, `--headless-test` flag, exit-status
mapping) these tests build on.

## Boot-time mapping smoke test (unconditional)

Every normal boot (`cargo run`, with or without any test feature) runs
`arch::x86_64::memory::run_boot_smoke_test()` immediately after
`arch::init(boot_info)` returns, before the framebuffer is rendered. It
verifies:

1. a non-canonical address is rejected (`MemoryError::NonCanonicalAddress`)
   without panicking;
2. a temporary test page is not already mapped;
3. mapping it (`Permissions::KERNEL_DATA`) succeeds and translating it
   reports the frame just allocated, with the writable and no-execute flags
   set and the user-accessible flag clear;
4. writing and reading back a sentinel value through the new mapping
   succeeds;
5. changing the mapping to `Permissions::KERNEL_READ_ONLY` succeeds;
6. unmapping it succeeds and returns the same frame that was mapped;
7. translating the address afterward reports it as unmapped again.

Expected output, right after PIT configuration and before the framebuffer
is initialized:

```
[memory] physical frame allocator initialized
[memory] usable regions: 95, allocatable 4 KiB frames: 128027
[memory] active page table initialized
[memory] paging mode: 4-level
[memory] NX: enabled
...
[memory] running temporary mapping smoke test
[memory] frames: 128027 usable, 4 allocated so far
[memory] temporary mapping smoke test passed
```

This test uses `assert!`/`assert_eq!`/`panic!` rather than returning a
`Result`: a broken mapping foundation is a fatal boot invariant violation in
this milestone, not a recoverable condition, and every other subsystem that
runs afterward implicitly depends on it working.

## Post-unmap page-fault test (`mem-fault-tests` feature)

Disabled by default. Deliberately maps a temporary page, writes and
verifies a sentinel value, unmaps it, and then re-accesses the now-unmapped
address — exercising the page-fault handler's not-present classification
end to end. Run it with:

```bash
cargo run --features mem-fault-tests
```

Expected output, after the timer verification and (if enabled)
`fault-tests`' own page fault:

```
[memory] running post-unmap page-fault test
[memory] WARNING: deliberately accessing unmapped page 0xffff900000000000
[exception] page fault
faulting address (CR2): 0xffff900000000000
error code: PageFaultErrorCode(0x0)
... (interrupt stack frame) ...
```

Without the `qemu-test` feature, the kernel halts in the page-fault handler
(`halt::halt_forever`), matching the existing `fault-tests` page-fault
test's behavior — this is expected, not a hang.

## Running the deterministic headless QEMU version

With `qemu-test` also enabled, `interrupts::page_fault_handler` recognizes
this specific fault (not-present error code, matching the exact address
just unmapped) and exits QEMU successfully instead of halting:

```bash
timeout 60 cargo run --features "qemu-test mem-fault-tests" -- --headless-test
```

Expected final lines:

```
[memory] running post-unmap page-fault test
[memory] WARNING: deliberately accessing unmapped page 0xffff900000000000
[exception] page fault
faulting address (CR2): 0xffff900000000000
error code: PageFaultErrorCode(0x0)
... (interrupt stack frame) ...
[test] expected post-unmap page fault matched
```

This can be combined with the existing tests, for example:

```bash
timeout 60 cargo run --features "qemu-test fault-tests mem-fault-tests" -- --headless-test
```

## Supervisor write-protection test (`supervisor-write-protection-test`)

This test maps a present, supervisor-only, read-only, non-executable page,
then deliberately writes to it from CPL0. The page-fault handler accepts
only a fault at the expected address with the present/protection-violation
and write bits set, the user-mode and instruction-fetch bits clear, and a
CPL0 origin. If the write succeeds or a different fault occurs, the test
fails. `memory::init` also establishes CR0.WP before paging initialization,
so the test exercises the architectural invariant rather than test-only
configuration.

Build the scenario with:

```bash
cargo build --features supervisor-write-protection-test
```

Run the deterministic QEMU test with:

```bash
timeout 60 cargo run --features "qemu-test supervisor-write-protection-test" -- --headless-test
```

The CI workflow runs this scenario alongside the paging and CPL3 fault
tests.

## CI integration

`.github/workflows/ci.yml` builds the relevant feature combinations in its
`Build` matrix and runs `QEMU / post-unmap page fault` on every pull request
and push to `main`, alongside the timer, expected page fault, supervisor
write-protection, CPL3, and kernel-heap scenarios.

## Kernel heap tests (pvKernel 0.0.7)

The allocator is initialized only after all 256 fixed heap pages are mapped
and the first, middle, last, and guard-page translations are verified. Boot
then exercises `Box`, capacity-growing `Vec`, `String`, alignments from 1
through 4096 bytes, freeing/reuse, and fragmentation with coalescing. CI
runs these success checks and two isolated scenarios:

```bash
cargo run --features "qemu-test heap-test" -- --headless-test
cargo run --features "qemu-test heap-oom-test" -- --headless-test
cargo run --features "qemu-test heap-guard-fault-test" -- --headless-test
```

The OOM case asks the allocator directly for more than twice the heap size
and expects null, without allocating from its failure path. The guard case
accesses the lower supervisor guard page and accepts only a not-present,
supervisor, read page fault at that exact address.
