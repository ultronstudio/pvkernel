//! Provisional bootstrap syscall error representation.
//!
//! Not borrowed from `errno.h` or any POSIX/Linux numbering - pvKernel does
//! not claim compatibility with either (see
//! [docs/architecture/syscalls.md](../../../../../../docs/architecture/syscalls.md)).
//! A bootstrap syscall's result is encoded entirely in `RAX`: bit 63 set
//! means "this is a [`SyscallError`]", clear means "this is a plain success
//! value", so every success value must stay within the remaining 63 bits.

/// Marks a `RAX` result as an error rather than a success value, in the
/// provisional bootstrap ABI: `RAX = ERROR_BIT | (error as u64)` on error,
/// `RAX = value` (with bit 63 clear) on success.
pub const ERROR_BIT: u64 = 1 << 63;

/// A provisional bootstrap syscall error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallError {
    /// The syscall number (`RAX` on entry) is not recognized.
    Unsupported = 1,
    /// An argument value was outside its documented valid range.
    ///
    /// Reserved: no bootstrap syscall in 0.0.6 currently validates an
    /// argument strictly enough to return this (`DEBUG_WRITE_BYTE` accepts
    /// any byte value, and the test-only syscalls don't validate their
    /// arguments either).
    #[allow(dead_code)]
    InvalidArgument = 2,
    /// The syscall is not permitted in the current context.
    ///
    /// Reserved: no bootstrap syscall in 0.0.6 currently returns this, since
    /// every implemented syscall is an unprivileged, always-allowed
    /// bootstrap operation.
    #[allow(dead_code)]
    PermissionDenied = 3,
    /// The `int 0x80` gate was entered from an unexpected execution
    /// context. See `super::dispatch::pvkernel_syscall_dispatch`'s
    /// CPL0-vs-CPL3 check: a CPL0 origin is treated as a kernel bug, not an
    /// ordinary user-triggerable error, but still returns this value
    /// (rather than panicking) so a stray kernel-mode `int 0x80` cannot
    /// itself crash the kernel.
    InvalidContext = 4,
}

impl SyscallError {
    /// Encodes this error into a `RAX` result value.
    pub const fn encode(self) -> u64 {
        ERROR_BIT | (self as u64)
    }
}
