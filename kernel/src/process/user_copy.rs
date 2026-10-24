//! Checked process-aware user-memory copies.
//!
//! System calls should use these entry points rather than dereferencing a
//! raw userspace pointer. Copying is page-chunked through the audited
//! supervisor physical map, with the entire mapping range checked first.

use crate::{arch::x86_64::memory::AddressSpaceError, task};

/// Copies from the current process into a kernel-owned buffer.
#[allow(dead_code)] // Public-in-module syscall API; getpid/exit do not carry user buffers.
pub fn copy_from_user(address: u64, output: &mut [u8]) -> Result<(), AddressSpaceError> {
    let process_id = task::current_user_process_id().ok_or(AddressSpaceError::InvalidCopyRange)?;
    super::copy_from_user(process_id, address, output)
}

/// Copies from a kernel-owned buffer into writable memory in the current
/// process.
#[allow(dead_code)] // Public-in-module syscall API; getpid/exit do not carry user buffers.
pub fn copy_to_user(address: u64, input: &[u8]) -> Result<(), AddressSpaceError> {
    let process_id = task::current_user_process_id().ok_or(AddressSpaceError::InvalidCopyRange)?;
    super::copy_to_user(process_id, address, input)
}
