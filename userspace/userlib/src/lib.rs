//! Minimal `no_std` userspace API for pvKernel ABI v1.
//!
//! The syscall numbers and register contract are documented in
//! `docs/architecture/syscalls.md`. This crate is the supported Rust wrapper
//! for that contract; it does not imply POSIX or Linux compatibility.

#![no_std]

const ABI_VERSION_QUERY: u64 = 0;
const DEBUG_WRITE_BYTE: u64 = 1;
const GETPID: u64 = 3;
const EXIT: u64 = 4;
const OPEN: u64 = 5;
const READ: u64 = 6;
const CLOSE: u64 = 7;
const CONSOLE_WRITE: u64 = 8;
const SPAWN: u64 = 9;
const WAIT_CHILD: u64 = 10;
const ERROR_BIT: u64 = 1 << 63;
const MAX_USER_IO: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Error {
    Unsupported = 1,
    InvalidArgument = 2,
    PermissionDenied = 3,
    InvalidContext = 4,
    BadDescriptor = 5,
    NotFound = 6,
    InvalidOperation = 7,
    NoResources = 8,
    UserMemory = 9,
    NotChild = 10,
    IoFailure = 11,
    Unknown = u64::MAX,
}

fn decode(result: u64) -> Result<u64, Error> {
    if result & ERROR_BIT == 0 {
        return Ok(result);
    }
    Err(match result & !ERROR_BIT {
        1 => Error::Unsupported,
        2 => Error::InvalidArgument,
        3 => Error::PermissionDenied,
        4 => Error::InvalidContext,
        5 => Error::BadDescriptor,
        6 => Error::NotFound,
        7 => Error::InvalidOperation,
        8 => Error::NoResources,
        9 => Error::UserMemory,
        10 => Error::NotChild,
        11 => Error::IoFailure,
        _ => Error::Unknown,
    })
}

fn syscall(number: u64, argument0: u64, argument1: u64, argument2: u64) -> Result<u64, Error> {
    let result: u64;
    // SAFETY: the ABI preserves all user GPRs other than RAX and treats
    // pointer arguments as untrusted; the kernel validates them before use.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") number => result,
            in("rdi") argument0,
            in("rsi") argument1,
            in("rdx") argument2,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    decode(result)
}

pub fn abi_version() -> Result<u64, Error> {
    syscall(ABI_VERSION_QUERY, 0, 0, 0)
}

pub fn process_id() -> Result<u64, Error> {
    syscall(GETPID, 0, 0, 0)
}

pub fn debug_write_byte(byte: u8) -> Result<(), Error> {
    syscall(DEBUG_WRITE_BYTE, u64::from(byte), 0, 0).map(|_| ())
}

/// Writes at most one ABI-sized chunk to the serial console.
pub fn console_write(bytes: &[u8]) -> Result<usize, Error> {
    if bytes.len() > MAX_USER_IO {
        return Err(Error::InvalidArgument);
    }
    let written = syscall(CONSOLE_WRITE, bytes.as_ptr() as u64, bytes.len() as u64, 0)?;
    usize::try_from(written).map_err(|_| Error::IoFailure)
}

/// A read-only file descriptor owned by the current process.
pub struct File {
    descriptor: u64,
}

impl File {
    pub fn open(path: &str) -> Result<Self, Error> {
        let descriptor = syscall(OPEN, path.as_ptr() as u64, path.len() as u64, 0)?;
        Ok(Self { descriptor })
    }

    /// Reads into a caller-owned buffer. The kernel caps each read at 512 bytes.
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        if output.len() > MAX_USER_IO {
            return Err(Error::InvalidArgument);
        }
        let count = syscall(
            READ,
            self.descriptor,
            output.as_mut_ptr() as u64,
            output.len() as u64,
        )?;
        usize::try_from(count).map_err(|_| Error::IoFailure)
    }

    pub fn close(self) -> Result<(), Error> {
        syscall(CLOSE, self.descriptor, 0, 0).map(|_| ())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessId(u64);

impl ProcessId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

pub fn spawn(path: &str) -> Result<ProcessId, Error> {
    syscall(SPAWN, path.as_ptr() as u64, path.len() as u64, 0).map(ProcessId)
}

/// Checks a direct child without blocking. Returns `false` while it runs;
/// on `true`, writes its exit status and reaps it.
pub fn wait_child(child: ProcessId, status: &mut i32) -> Result<bool, Error> {
    Ok(syscall(WAIT_CHILD, child.0, status as *mut i32 as u64, 0)? == 1)
}

pub fn exit(status: i32) -> ! {
    let _ = syscall(EXIT, status as u32 as u64, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}
