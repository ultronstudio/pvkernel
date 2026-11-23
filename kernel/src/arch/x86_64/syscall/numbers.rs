//! pvOS ABI v1 syscall numbers (the value placed in `RAX` before `int 0x80`).
//!
//! This is pvKernel's own ABI, not POSIX or Linux. Existing numbers and
//! meanings are preserved from the 0.1.0 compatibility checkpoint onward;
//! additions use unused numbers. See
//! [docs/architecture/syscalls.md](../../../../../../docs/architecture/syscalls.md).

/// The pvOS userspace ABI version returned by [`ABI_VERSION_QUERY`].
///
/// Identifies the register convention, error encoding, and syscall behavior.
/// Additive syscall numbers preserve v1; incompatible changes require a bump.
pub const ABI_VERSION: u64 = 1;

/// Queries the userspace ABI version.
///
/// Arguments: none. Returns: [`ABI_VERSION`] in `RAX`.
pub const ABI_VERSION_QUERY: u64 = 0;

/// Writes one byte to the kernel's serial diagnostic output.
///
/// Arguments: `RDI` bits `7:0` are the byte to write; bits `63:8` are
/// ignored. Returns: `0` in `RAX`.
///
/// This retained diagnostic syscall intentionally takes its byte by value.
/// Buffer output is provided by [`CONSOLE_WRITE`]. See
/// docs/architecture/syscalls.md.
pub const DEBUG_WRITE_BYTE: u64 = 1;

/// Reads the kernel's PIT tick counter.
///
/// Arguments: none. Returns: `arch::x86_64::pit::ticks()` in `RAX`.
pub const TIMER_TICKS: u64 = 2;

/// Returns the current process's human-visible PID, or `InvalidContext` for
/// bootstrap/kernel execution without a process-owned user Task.
pub const GETPID: u64 = 3;

/// Terminates the current process with `RDI as i32`. This syscall never
/// returns to its caller; process exit is completed after a timer handoff.
pub const EXIT: u64 = 4;

/// Opens a read-only regular file by a UTF-8, volume-qualified path.
/// Arguments: `RDI` user pointer, `RSI` byte length. Returns a descriptor >= 3.
pub const OPEN: u64 = 5;

/// Reads from a descriptor. Arguments: `RDI` descriptor, `RSI` output pointer,
/// `RDX` maximum byte count. Returns bytes copied, or zero at end of file.
pub const READ: u64 = 6;

/// Closes a descriptor owned by the current process. Argument: `RDI` descriptor.
pub const CLOSE: u64 = 7;

/// Writes a bounded byte buffer to the serial console.
/// Arguments: `RDI` user pointer, `RSI` byte length. Returns bytes written.
pub const CONSOLE_WRITE: u64 = 8;

/// Loads a read-only ELF file from VFS as a child of the current process.
/// Arguments: `RDI` path pointer, `RSI` path length. Returns child PID.
pub const SPAWN: u64 = 9;

/// Non-blocking direct-child wait. Arguments: `RDI` child PID, `RSI` status
/// output pointer. Returns 0 while running, 1 after writing status and reaping.
pub const WAIT_CHILD: u64 = 10;

/// Start of the test-only syscall namespace.
///
/// Numbers at or above this value are only ever dispatched when the
/// `userspace-test` Cargo feature is enabled (see `super::dispatch`); they
/// are never part of the documented bootstrap ABI above, and a build
/// without `userspace-test` treats them exactly like any other unknown
/// number ([`super::error::SyscallError::Unsupported`]).
///
/// Kept in sync with the raw immediate baked into the hand-written test
/// payloads in `arch::x86_64::userspace::payload`; see the compile-time
/// assertions there.
#[cfg(feature = "userspace-test")]
pub const TEST_NAMESPACE_BASE: u64 = 0x1000;

/// Test-only: reports observed test conditions as a bitset in `RAX`.
///
/// Arguments: none. Currently defines only
/// [`TEST_STATUS_TIMER_INTERRUPTED_CPL3`].
#[cfg(feature = "userspace-test")]
pub const TEST_STATUS: u64 = TEST_NAMESPACE_BASE;

/// Bit set in [`TEST_STATUS`]'s result once a PIT timer interrupt has been
/// observed to interrupt CPL3 execution at least once. See
/// `arch::x86_64::syscall::test`.
#[cfg(feature = "userspace-test")]
pub const TEST_STATUS_TIMER_INTERRUPTED_CPL3: u64 = 1 << 0;

/// Test-only: ends the current deterministic userspace test. Never returns.
///
/// Arguments: `RDI` is `0` for success, non-zero for failure.
#[cfg(feature = "userspace-test")]
pub const TEST_EXIT: u64 = TEST_NAMESPACE_BASE + 1;
