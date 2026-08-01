//! Provisional bootstrap syscall numbers (the value placed in `RAX` before
//! `int 0x80`).
//!
//! This is pvKernel's own **provisional** ABI: it is not POSIX, not Linux,
//! and not the eventual stable pvOS syscall ABI. It exists only so 0.0.6 has
//! *something* concrete to test the CPL3-to-CPL0 transition, the RSP0
//! ring-transition stack, and register preservation against. Expect the
//! encoding, numbering, and argument conventions here to be replaced once a
//! real per-process syscall entry mechanism exists. See
//! [docs/architecture/syscalls.md](../../../../../../docs/architecture/syscalls.md).

/// The provisional bootstrap ABI version returned by [`ABI_VERSION_QUERY`].
///
/// Identifies the shape of *this* `int 0x80` bootstrap ABI (register
/// convention, error encoding, this syscall number set) - not a pvOS or
/// POSIX ABI version. Bump this if the bootstrap ABI's shape ever changes
/// in an incompatible way before it is replaced entirely.
pub const ABI_VERSION: u64 = 1;

/// Queries the provisional ABI version.
///
/// Arguments: none. Returns: [`ABI_VERSION`] in `RAX`.
pub const ABI_VERSION_QUERY: u64 = 0;

/// Writes one byte to the kernel's serial diagnostic output.
///
/// Arguments: `RDI` bits `7:0` are the byte to write; bits `63:8` are
/// ignored. Returns: `0` in `RAX`.
///
/// This intentionally takes the byte **by value**, not by pointer: 0.0.6
/// has no `copy_from_user` and no policy for validating a user-supplied
/// pointer, so no syscall in this milestone ever dereferences one. See
/// docs/architecture/syscalls.md.
pub const DEBUG_WRITE_BYTE: u64 = 1;

/// Reads the kernel's PIT tick counter.
///
/// Arguments: none. Returns: `arch::x86_64::pit::ticks()` in `RAX`.
pub const TIMER_TICKS: u64 = 2;

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
