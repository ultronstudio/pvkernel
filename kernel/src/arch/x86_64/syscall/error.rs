//! pvOS ABI v1 syscall error representation.
//!
//! Not borrowed from `errno.h` or any POSIX/Linux numbering - pvKernel does
//! not claim compatibility with either (see
//! [docs/architecture/syscalls.md](../../../../../../docs/architecture/syscalls.md)).
//! A bootstrap syscall's result is encoded entirely in `RAX`: bit 63 set
//! means "this is a [`SyscallError`]", clear means "this is a plain success
//! value", so every success value must stay within the remaining 63 bits.

/// Marks a `RAX` result as an error rather than a success value, in the
/// ABI v1: `RAX = ERROR_BIT | (error as u64)` on error,
/// `RAX = value` (with bit 63 clear) on success.
pub const ERROR_BIT: u64 = 1 << 63;

/// A syscall error with a stable ABI v1 numeric representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallError {
    /// The syscall number (`RAX` on entry) is not recognized.
    Unsupported = 1,
    /// An argument value was outside its documented valid range.
    InvalidArgument = 2,
    /// The syscall is not permitted in the current context.
    #[allow(dead_code)] // Reserved ABI v1 value; current calls use more specific errors.
    PermissionDenied = 3,
    /// The `int 0x80` gate was entered from an unexpected execution
    /// context. See `super::dispatch::pvkernel_syscall_dispatch`'s
    /// CPL0-vs-CPL3 check: a CPL0 origin is treated as a kernel bug, not an
    /// ordinary user-triggerable error, but still returns this value
    /// (rather than panicking) so a stray kernel-mode `int 0x80` cannot
    /// itself crash the kernel.
    InvalidContext = 4,
    /// Descriptor does not name an open file owned by this process.
    BadDescriptor = 5,
    /// The path or child PID did not resolve in the caller's namespace.
    NotFound = 6,
    /// The requested operation is invalid for a directory or process state.
    InvalidOperation = 7,
    /// A bounded kernel table has no available slots.
    NoResources = 8,
    /// The user buffer is not fully accessible with the required permissions.
    UserMemory = 9,
    /// The requested PID is not a direct child of the caller.
    NotChild = 10,
    /// The VFS provider failed while servicing a read.
    IoFailure = 11,
}

impl SyscallError {
    /// Encodes this error into a `RAX` result value.
    pub const fn encode(self) -> u64 {
        ERROR_BIT | (self as u64)
    }
}
