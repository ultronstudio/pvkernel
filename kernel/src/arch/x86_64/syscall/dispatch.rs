//! Rust-side `int 0x80` syscall dispatcher.
//!
//! [`pvkernel_syscall_dispatch`] is called only from `super::entry`'s
//! naked-asm stub, through the `extern "C"` boundary documented there. It
//! is never called directly from ordinary kernel code, and never called
//! with a frame that did not just come from a live `int 0x80`.

use x86_64::PrivilegeLevel;

use super::error::SyscallError;
use super::frame::SyscallFrame;
use super::numbers;
use crate::kprintln;

const MAX_USER_PATH: usize = pvkernel_fs_core::path::MAX_PATH_LEN;
const MAX_USER_IO: usize = 512;

/// Dispatches one `int 0x80` syscall and returns its `RAX`-encoded result.
///
/// # Safety
///
/// `frame` must point to a valid, currently-live [`SyscallFrame`] on the
/// RSP0 stack, exactly as constructed by `super::entry`'s naked-asm stub
/// immediately before this call. This function only reads scalar integer
/// fields out of it and never retains the pointer past this call.
pub(super) extern "C" fn pvkernel_syscall_dispatch(frame: *const SyscallFrame) -> u64 {
    // SAFETY: forwarded from this function's own contract above, upheld by
    // its sole caller (`super::entry`'s naked-asm stub).
    let frame = unsafe { &*frame };

    // Confirms the entry stub actually built this frame on the dedicated
    // RSP0 ring-transition stack (see `gdt::syscall_stack_top`) rather than
    // some other stack - only meaningful under `userspace-test`, since only
    // its deterministic scenarios control the full call path closely enough
    // for this to be a useful invariant rather than noise.
    #[cfg(feature = "userspace-test")]
    {
        let (bottom, top) = super::super::gdt::rsp0_stack_range();
        let frame_addr = frame as *const SyscallFrame as u64;
        debug_assert!(
            frame_addr >= bottom && frame_addr < top,
            "syscall frame at {:#x} is outside the RSP0 stack range [{:#x}, {:#x})",
            frame_addr,
            bottom,
            top
        );
    }

    if frame.caller_privilege_level() != PrivilegeLevel::Ring3 {
        // No supported userspace ABI call invokes `int 0x80` from
        // CPL0; this is a kernel bug, not an ordinary user-triggerable
        // error. It is still reported through the normal error-return path
        // (rather than panicking) so that a stray kernel-mode `int 0x80`
        // cannot itself crash the kernel - but it is loud on serial output
        // so the bug is not silently swallowed either.
        kprintln!(
            "[syscall] BUG: int 0x80 invoked from CPL{} (expected CPL3)",
            frame.cs & 0b11
        );
        return SyscallError::InvalidContext.encode();
    }

    // Every argument register is treated as an opaque integer until a
    // specific syscall validates it. Pointer-taking calls use user_copy.
    let number = frame.rax;
    let arg0 = frame.rdi;

    match number {
        numbers::ABI_VERSION_QUERY => numbers::ABI_VERSION,
        numbers::DEBUG_WRITE_BYTE => debug_write_byte(arg0),
        numbers::TIMER_TICKS => crate::arch::x86_64::pit::ticks(),
        numbers::GETPID => {
            crate::task::current_user_pid().unwrap_or_else(|| SyscallError::InvalidContext.encode())
        }
        numbers::EXIT => crate::task::exit_current_user(frame.rdi as u32 as i32),
        numbers::OPEN => open_file(frame.rdi, frame.rsi),
        numbers::READ => read_file(frame.rdi, frame.rsi, frame.rdx),
        numbers::CLOSE => close_file(frame.rdi),
        numbers::CONSOLE_WRITE => console_write(frame.rdi, frame.rsi),
        numbers::SPAWN => execute_path(frame.rdi, frame.rsi),
        numbers::WAIT_CHILD => wait_child(frame.rdi, frame.rsi),
        #[cfg(feature = "userspace-test")]
        n if n >= numbers::TEST_NAMESPACE_BASE => super::test::dispatch(n, frame),
        _ => SyscallError::Unsupported.encode(),
    }
}

fn current_process() -> Result<crate::process::ProcessId, u64> {
    crate::task::current_user_process_id().ok_or(SyscallError::InvalidContext.encode())
}

fn copy_user_path(address: u64, length: u64, bytes: &mut [u8; MAX_USER_PATH]) -> Result<&str, u64> {
    let length = usize::try_from(length).map_err(|_| SyscallError::InvalidArgument.encode())?;
    if length == 0 || length > bytes.len() {
        return Err(SyscallError::InvalidArgument.encode());
    }
    crate::process::user_copy::copy_from_user(address, &mut bytes[..length])
        .map_err(|_| SyscallError::UserMemory.encode())?;
    let path = core::str::from_utf8(&bytes[..length])
        .map_err(|_| SyscallError::InvalidArgument.encode())?;
    Ok(path)
}

fn map_fs_error(error: crate::fs::vfs::FsError) -> u64 {
    use crate::fs::vfs::FsError;
    match error {
        FsError::NotFound | FsError::VolumeNotFound => SyscallError::NotFound.encode(),
        FsError::IsDirectory => SyscallError::InvalidOperation.encode(),
        FsError::InvalidPath | FsError::InvalidVolumeName | FsError::InvalidOffset => {
            SyscallError::InvalidArgument.encode()
        }
        _ => SyscallError::IoFailure.encode(),
    }
}

fn map_file_error(error: crate::process::files::FileError) -> u64 {
    use crate::process::files::FileError;
    match error {
        FileError::BadDescriptor => SyscallError::BadDescriptor.encode(),
        FileError::IsDirectory => SyscallError::InvalidOperation.encode(),
        FileError::TableFull => SyscallError::NoResources.encode(),
        FileError::Filesystem(error) => map_fs_error(error),
    }
}

fn open_file(address: u64, length: u64) -> u64 {
    let owner = match current_process() {
        Ok(owner) => owner,
        Err(error) => return error,
    };
    let mut bytes = [0; MAX_USER_PATH];
    let path = match copy_user_path(address, length, &mut bytes) {
        Ok(path) => path,
        Err(error) => return error,
    };
    crate::process::files::open(owner, path).unwrap_or_else(map_file_error)
}

fn read_file(descriptor: u64, address: u64, length: u64) -> u64 {
    let owner = match current_process() {
        Ok(owner) => owner,
        Err(error) => return error,
    };
    let length = match usize::try_from(length) {
        Ok(length) if length <= MAX_USER_IO => length,
        _ => return SyscallError::InvalidArgument.encode(),
    };
    let (node, offset) = match crate::process::files::read_plan(owner, descriptor) {
        Ok(plan) => plan,
        Err(error) => return map_file_error(error),
    };
    let mut bytes = [0; MAX_USER_IO];
    let count = match node.read(offset, &mut bytes[..length]) {
        Ok(count) => count,
        Err(error) => return map_fs_error(error),
    };
    if count != 0 && crate::process::user_copy::copy_to_user(address, &bytes[..count]).is_err() {
        return SyscallError::UserMemory.encode();
    }
    if let Err(error) = crate::process::files::commit_read(owner, descriptor, offset, count) {
        return map_file_error(error);
    }
    count as u64
}

fn close_file(descriptor: u64) -> u64 {
    let owner = match current_process() {
        Ok(owner) => owner,
        Err(error) => return error,
    };
    crate::process::files::close(owner, descriptor).map_or_else(map_file_error, |_| 0)
}

fn console_write(address: u64, length: u64) -> u64 {
    if let Err(error) = current_process() {
        return error;
    }
    let length = match usize::try_from(length) {
        Ok(length) if length <= MAX_USER_IO => length,
        _ => return SyscallError::InvalidArgument.encode(),
    };
    if length == 0 {
        return 0;
    }
    let mut bytes = [0; MAX_USER_IO];
    if crate::process::user_copy::copy_from_user(address, &mut bytes[..length]).is_err() {
        return SyscallError::UserMemory.encode();
    }
    crate::drivers::serial::write_bytes(&bytes[..length]) as u64
}

fn execute_path(address: u64, length: u64) -> u64 {
    let parent = match current_process() {
        Ok(parent) => parent,
        Err(error) => return error,
    };
    let mut bytes = [0; MAX_USER_PATH];
    let path = match copy_user_path(address, length, &mut bytes) {
        Ok(path) => path,
        Err(error) => return error,
    };
    #[cfg(any(
        not(any(feature = "qemu-test", feature = "userspace-test")),
        feature = "initrd-boot-test"
    ))]
    {
        match crate::fs::vfs::spawn_path(Some(parent), path) {
            Ok(id) => crate::process::pid(id)
                .map(|pid| pid.get())
                .unwrap_or_else(|_| SyscallError::InvalidContext.encode()),
            Err(crate::fs::vfs::SpawnPathError::Filesystem(error)) => map_fs_error(error),
            Err(crate::fs::vfs::SpawnPathError::NotRegular) => {
                SyscallError::InvalidOperation.encode()
            }
            Err(crate::fs::vfs::SpawnPathError::InvalidName) => {
                SyscallError::InvalidArgument.encode()
            }
            Err(crate::fs::vfs::SpawnPathError::Process(_)) => SyscallError::NoResources.encode(),
        }
    }
    #[cfg(not(any(
        not(any(feature = "qemu-test", feature = "userspace-test")),
        feature = "initrd-boot-test"
    )))]
    {
        let _ = (parent, path);
        SyscallError::Unsupported.encode()
    }
}

fn wait_child(child_pid: u64, status_address: u64) -> u64 {
    let parent = match current_process() {
        Ok(parent) => parent,
        Err(error) => return error,
    };
    let (child, status) = match crate::process::try_wait_child(parent, child_pid) {
        Ok(Some(child)) => child,
        Ok(None) => return 0,
        Err(crate::process::ProcessError::NotChild) => return SyscallError::NotChild.encode(),
        Err(_) => return SyscallError::InvalidOperation.encode(),
    };
    if crate::process::user_copy::copy_to_user(status_address, &status.to_le_bytes()).is_err() {
        return SyscallError::UserMemory.encode();
    }
    if crate::process::reap_child(parent, child).is_err() {
        return SyscallError::InvalidOperation.encode();
    }
    1
}

/// Implements [`numbers::DEBUG_WRITE_BYTE`].
fn debug_write_byte(byte: u64) -> u64 {
    let init_pid_marker =
        byte as u8 == b'I' && crate::task::current_user_pid().is_some_and(|pid| pid == 1);
    if init_pid_marker {
        kprintln!("[init] entered CPL3");
    }
    kprintln!("[syscall] debug byte from CPL3: {:#04x}", byte as u8);
    // PID 1 deliberately remains alive in the real init image. The dedicated
    // boot-chain test therefore reports success at the marker instead of
    // waiting for kernel_main's bootstrap context to resume from the scheduler.
    #[cfg(feature = "initrd-boot-test")]
    if init_pid_marker {
        kprintln!("[initrd-test] PID 1 CPL3 marker accepted");
        crate::arch::x86_64::qemu::exit_success();
    }
    0
}
