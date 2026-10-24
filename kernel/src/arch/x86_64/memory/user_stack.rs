//! Fixed, guarded initial userspace stack layout for 0.0.9.

use x86_64::{
    VirtAddr,
    structures::paging::{Page, PageSize, Size4KiB},
};

use super::{AddressSpace, AddressSpaceError, KernelAddressSpace, Permissions, Translation};

pub const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_f000;
pub const USER_STACK_PAGES: u64 = 8;
pub const USER_STACK_SIZE: u64 = USER_STACK_PAGES * Size4KiB::SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserStack {
    pub bottom: u64,
    pub top: u64,
    pub guard: u64,
}

impl UserStack {
    /// Maps and zeroes the fixed 32 KiB USER RW/NX stack. The lower guard is
    /// left unmapped. On failure, already installed leaves are removed; the
    /// monotonic allocator retains their physical frames by design.
    pub fn map(
        address_space: &mut AddressSpace,
        kernel: &mut KernelAddressSpace,
    ) -> Result<Self, AddressSpaceError> {
        let bottom = USER_STACK_TOP
            .checked_sub(USER_STACK_SIZE)
            .ok_or(AddressSpaceError::OutsideUserRange)?;
        let guard = bottom
            .checked_sub(Size4KiB::SIZE)
            .ok_or(AddressSpaceError::OutsideUserRange)?;
        if guard < super::USER_MIN || USER_STACK_TOP > super::USER_MAX {
            return Err(AddressSpaceError::OutsideUserRange);
        }
        let guard_page = Page::<Size4KiB>::from_start_address(VirtAddr::new(guard))
            .map_err(|_| AddressSpaceError::OutsideUserRange)?;
        if !matches!(
            address_space.translate(kernel, guard_page.start_address())?,
            Translation::NotMapped
        ) {
            return Err(AddressSpaceError::StackCollision);
        }

        let mut mapped = 0_u64;
        while mapped < USER_STACK_PAGES {
            let offset = mapped
                .checked_mul(Size4KiB::SIZE)
                .ok_or(AddressSpaceError::OutsideUserRange)?;
            let address = bottom
                .checked_add(offset)
                .ok_or(AddressSpaceError::OutsideUserRange)?;
            let page = Page::<Size4KiB>::from_start_address(VirtAddr::new(address))
                .map_err(|_| AddressSpaceError::OutsideUserRange)?;
            if !matches!(
                address_space.translate(kernel, VirtAddr::new(address))?,
                Translation::NotMapped
            ) {
                rollback(address_space, kernel, bottom, mapped)?;
                return Err(AddressSpaceError::StackCollision);
            }
            if let Err(error) =
                address_space.map_new_user_page(kernel, page, Permissions::USER_DATA)
            {
                rollback(address_space, kernel, bottom, mapped)?;
                return Err(error);
            }
            mapped += 1;
        }
        Ok(Self {
            bottom,
            top: USER_STACK_TOP,
            guard,
        })
    }
}

fn rollback(
    address_space: &mut AddressSpace,
    kernel: &mut KernelAddressSpace,
    bottom: u64,
    count: u64,
) -> Result<(), AddressSpaceError> {
    for index in 0..count {
        let offset = index
            .checked_mul(Size4KiB::SIZE)
            .ok_or(AddressSpaceError::OutsideUserRange)?;
        let address = bottom
            .checked_add(offset)
            .ok_or(AddressSpaceError::OutsideUserRange)?;
        let page = Page::<Size4KiB>::from_start_address(VirtAddr::new(address))
            .map_err(|_| AddressSpaceError::OutsideUserRange)?;
        address_space.unmap_user_page(kernel, page)?;
    }
    Ok(())
}
