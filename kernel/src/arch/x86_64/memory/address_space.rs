//! Ownership of the active x86_64 page table and the boot frame allocator.
//!
//! The single [`KernelAddressSpace`] owns the kernel root and allocator. It
//! is built once during [`super::init`] and afterward is reachable through
//! [`with_kernel_address_space`], which serializes access behind a
//! [`spin::Mutex`] and disables maskable interrupts for the duration of the
//! call. Process [`AddressSpace`] values own inactive roots whose audited
//! upper-half subtrees are shared with the kernel root.

use spin::{Mutex, Once};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::{Cr3, Cr3Flags},
    structures::paging::{
        FrameAllocator, OffsetPageTable, Page, PageSize, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
};

use super::frame_allocator::BootFrameAllocator;
use super::paging::{self, MemoryError, Permissions, Translation};

static KERNEL_ADDRESS_SPACE: Once<Mutex<KernelAddressSpace>> = Once::new();

#[derive(Clone, Copy, Debug)]
pub(super) struct MappingRange {
    pub(super) start: u64,
    pub(super) len: u64,
}

#[derive(Clone, Copy)]
pub(super) struct BootMappingInventory {
    pub(super) kernel_image_base: u64,
    pub(super) kernel_elf_physical: u64,
    pub(super) kernel_elf_len: u64,
    pub(super) kernel_stack: MappingRange,
    pub(super) boot_info: MappingRange,
    pub(super) memory_regions: MappingRange,
    pub(super) physical_memory: MappingRange,
    pub(super) framebuffer: Option<MappingRange>,
    pub(super) ramdisk: Option<MappingRange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressSpaceError {
    FrameAllocationFailed,
    InvalidRoot,
    UnsafeKernelMapping,
    KernelMappingMissing,
    OutsideUserRange,
    NonUserPermissions,
    UserPageAlreadyMapped,
    UserPageNotMapped,
    StackCollision,
    Mapping(MemoryError),
    AccountingOverflow,
    InvalidCopyRange,
    InvalidUserContext,
}

impl From<MemoryError> for AddressSpaceError {
    fn from(error: MemoryError) -> Self {
        Self::Mapping(error)
    }
}

/// One independent four-level root. Its lower-half page tables and user
/// frames are process-owned; audited upper-half kernel PML4 entries share the
/// kernel's supervisor-only subtrees. Frames remain monotonic/kernel-lifetime
/// owned until general reclamation exists.
pub struct AddressSpace {
    root: PhysFrame<Size4KiB>,
    mapped_user_pages: u64,
}

/// Opaque, copyable identity for a process root used only by the architecture
/// switch boundary. It exposes neither a naked CR3 integer nor page-table
/// mutation to generic scheduler code. The owning Process must stay Alive (or
/// Exiting while its task is active) until another context has been adopted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressSpaceHandle(PhysFrame<Size4KiB>);

/// User entry/stack pair checked against one specific inactive root. Fields
/// remain private so callers cannot fabricate a context from raw addresses.
#[derive(Clone, Copy)]
pub struct ValidatedUserContext {
    root: PhysFrame<Size4KiB>,
    entry: u64,
    user_rsp: u64,
}

impl ValidatedUserContext {
    pub const fn entry(self) -> u64 {
        self.entry
    }
    pub const fn user_rsp(self) -> u64 {
        self.user_rsp
    }
    pub(crate) const fn root_frame(self) -> PhysFrame<Size4KiB> {
        self.root
    }
}

impl AddressSpace {
    pub const fn mapped_user_pages(&self) -> u64 {
        self.mapped_user_pages
    }

    pub(crate) fn root_frame(&self) -> PhysFrame<Size4KiB> {
        self.root
    }

    pub(crate) const fn handle(&self) -> AddressSpaceHandle {
        AddressSpaceHandle(self.root)
    }

    pub fn validate_user_context(
        &mut self,
        kernel: &mut KernelAddressSpace,
        entry: u64,
        user_rsp: u64,
    ) -> Result<ValidatedUserContext, AddressSpaceError> {
        if entry < super::USER_MIN
            || entry >= super::USER_MAX
            || user_rsp <= super::USER_MIN
            || user_rsp > super::USER_MAX
            || user_rsp % 16 != 0
            || VirtAddr::try_new(entry).is_err()
            || VirtAddr::try_new(user_rsp).is_err()
        {
            return Err(AddressSpaceError::InvalidUserContext);
        }
        match self.translate(kernel, VirtAddr::new(entry))? {
            Translation::Mapped { flags, .. }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                    && !flags.contains(PageTableFlags::WRITABLE)
                    && !flags.contains(PageTableFlags::NO_EXECUTE) => {}
            _ => return Err(AddressSpaceError::InvalidUserContext),
        }
        let stack_byte = user_rsp
            .checked_sub(1)
            .ok_or(AddressSpaceError::InvalidUserContext)?;
        match self.translate(kernel, VirtAddr::new(stack_byte))? {
            Translation::Mapped { flags, .. }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                    && flags.contains(PageTableFlags::WRITABLE)
                    && flags.contains(PageTableFlags::NO_EXECUTE) => {}
            _ => return Err(AddressSpaceError::InvalidUserContext),
        }
        Ok(ValidatedUserContext {
            root: self.root,
            entry,
            user_rsp,
        })
    }

    pub fn map_new_user_page(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
        permissions: Permissions,
    ) -> Result<PhysFrame<Size4KiB>, AddressSpaceError> {
        validate_user_page(page, permissions)?;
        if !matches!(
            self.translate(kernel, page.start_address())?,
            Translation::NotMapped
        ) {
            return Err(AddressSpaceError::UserPageAlreadyMapped);
        }
        let next_count = self
            .mapped_user_pages
            .checked_add(1)
            .ok_or(AddressSpaceError::AccountingOverflow)?;
        let frame = kernel.allocate_zeroed_frame()?;
        kernel.with_inactive_mapper(self, |mapper, allocator| {
            paging::map_page(mapper, allocator, page, frame, permissions)
        })?;
        self.mapped_user_pages = next_count;
        match self.translate(kernel, page.start_address())? {
            Translation::Mapped { flags, .. }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE) =>
            {
                Ok(frame)
            }
            _ => {
                let _ = self.unmap_user_page(kernel, page);
                Err(AddressSpaceError::NonUserPermissions)
            }
        }
    }

    /// Installs a supervisor-only PTE inside the user range for the
    /// user-copy negative test. This mapping is never exposed to a process
    /// task and is not part of the production mapping API.
    #[cfg(feature = "user-copy-test")]
    pub(crate) fn map_test_supervisor_page(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
    ) -> Result<(), AddressSpaceError> {
        validate_user_address(page.start_address())?;
        if !matches!(
            self.translate(kernel, page.start_address())?,
            Translation::NotMapped
        ) {
            return Err(AddressSpaceError::UserPageAlreadyMapped);
        }
        let frame = kernel.allocate_zeroed_frame()?;
        kernel.with_inactive_mapper(self, |mapper, allocator| {
            paging::map_page(mapper, allocator, page, frame, Permissions::KERNEL_DATA)
        })?;
        Ok(())
    }

    /// Test-only shared user page mapping for the CPL3 preemption handshake.
    /// This is not a production shared-memory API; it maps only the frame
    /// allocated by the paired deterministic QEMU test.
    #[cfg(feature = "process-preemption-test")]
    pub(crate) fn map_test_shared_user_page(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
    ) -> Result<(), AddressSpaceError> {
        validate_user_page(page, Permissions::USER_DATA)?;
        if !matches!(
            self.translate(kernel, page.start_address())?,
            Translation::NotMapped
        ) {
            return Err(AddressSpaceError::UserPageAlreadyMapped);
        }
        let next_count = self
            .mapped_user_pages
            .checked_add(1)
            .ok_or(AddressSpaceError::AccountingOverflow)?;
        kernel.with_inactive_mapper(self, |mapper, allocator| {
            paging::map_page(mapper, allocator, page, frame, Permissions::USER_DATA)
        })?;
        self.mapped_user_pages = next_count;
        Ok(())
    }

    pub fn update_user_permissions(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
        permissions: Permissions,
    ) -> Result<(), AddressSpaceError> {
        validate_user_page(page, permissions)?;
        match self.translate(kernel, page.start_address())? {
            Translation::Mapped { flags, .. }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE) => {}
            _ => return Err(AddressSpaceError::UserPageNotMapped),
        }
        kernel.with_inactive_mapper(self, |mapper, _| {
            paging::update_permissions(mapper, page, permissions)
        })?;
        Ok(())
    }

    /// Copies into one already-mapped, writable user page through the
    /// supervisor physical map. Callers split multi-page data into page
    /// chunks and keep the process Creating until initialization is complete.
    pub fn write_user_page(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), AddressSpaceError> {
        validate_user_address(page.start_address())?;
        let end = offset
            .checked_add(bytes.len())
            .ok_or(AddressSpaceError::InvalidCopyRange)?;
        if end > Size4KiB::SIZE as usize {
            return Err(AddressSpaceError::InvalidCopyRange);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let virtual_address = page
            .start_address()
            .as_u64()
            .checked_add(offset as u64)
            .ok_or(AddressSpaceError::InvalidCopyRange)?;
        let physical = match self.translate(kernel, VirtAddr::new(virtual_address))? {
            Translation::Mapped { phys_addr, flags }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                    && flags.contains(PageTableFlags::WRITABLE) =>
            {
                phys_addr
            }
            _ => return Err(AddressSpaceError::UserPageNotMapped),
        };
        let target = kernel.physical_alias(physical)?;
        // SAFETY: the destination PTE belongs to this inactive AddressSpace,
        // was checked user-accessible and writable above, and the matching
        // physical frame is mapped supervisor-only through the audited
        // physical map. The input slice is valid for `bytes.len()`.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), target.as_mut_ptr::<u8>(), bytes.len());
        }
        Ok(())
    }

    /// Copies bytes from one mapped user page through its supervisor-only
    /// physical alias. The requested range must remain within that page.
    pub fn read_user_page(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), AddressSpaceError> {
        validate_user_address(page.start_address())?;
        let end = offset
            .checked_add(output.len())
            .ok_or(AddressSpaceError::InvalidCopyRange)?;
        if end > Size4KiB::SIZE as usize {
            return Err(AddressSpaceError::InvalidCopyRange);
        }
        if output.is_empty() {
            return Ok(());
        }
        let virtual_address = page
            .start_address()
            .as_u64()
            .checked_add(offset as u64)
            .ok_or(AddressSpaceError::InvalidCopyRange)?;
        let physical = match self.translate(kernel, VirtAddr::new(virtual_address))? {
            Translation::Mapped { phys_addr, flags }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE) =>
            {
                phys_addr
            }
            _ => return Err(AddressSpaceError::UserPageNotMapped),
        };
        let source = kernel.physical_alias(physical)?;
        // SAFETY: translation proved that the user mapping resolves to the
        // checked page, physical_alias proved a supervisor-readable mapping,
        // and output is a valid uniquely borrowed destination slice.
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.as_ptr::<u8>(),
                output.as_mut_ptr(),
                output.len(),
            );
        }
        Ok(())
    }

    /// Copies a complete userspace range into a kernel buffer without ever
    /// dereferencing the userspace virtual address. The full range is
    /// validated before bytes are copied, so an invalid later page cannot
    /// produce a partial kernel buffer update.
    ///
    /// This is safe for the 0.0.9 single-core model: user mappings cannot be
    /// changed while a syscall runs (IF is clear), and there is no user
    /// remapping, userspace threading, or SMP. Those assumptions must be
    /// revisited before adding any such feature.
    pub fn copy_from_user(
        &mut self,
        kernel: &mut KernelAddressSpace,
        user_address: u64,
        output: &mut [u8],
    ) -> Result<(), AddressSpaceError> {
        let range = validate_copy_range(user_address, output.len())?;
        if output.is_empty() {
            return Ok(());
        }
        self.validate_copy_pages(kernel, range, false)?;
        self.copy_user_chunks(
            kernel,
            range,
            output.len(),
            output.as_ptr(),
            output.as_mut_ptr(),
            CopyDirection::FromUser,
        )
    }

    /// Copies a complete kernel buffer into a writable userspace range
    /// through supervisor-only physical aliases. No raw user virtual pointer
    /// is passed to memcpy/copy_nonoverlapping.
    pub fn copy_to_user(
        &mut self,
        kernel: &mut KernelAddressSpace,
        user_address: u64,
        input: &[u8],
    ) -> Result<(), AddressSpaceError> {
        let range = validate_copy_range(user_address, input.len())?;
        if input.is_empty() {
            return Ok(());
        }
        self.validate_copy_pages(kernel, range, true)?;
        self.copy_user_chunks(
            kernel,
            range,
            input.len(),
            input.as_ptr(),
            core::ptr::null_mut(),
            CopyDirection::ToUser,
        )
    }

    fn validate_copy_pages(
        &mut self,
        kernel: &mut KernelAddressSpace,
        range: CopyRange,
        writable: bool,
    ) -> Result<(), AddressSpaceError> {
        let mut address = range.start;
        while address < range.end {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
            match self.translate(kernel, VirtAddr::new(address))? {
                Translation::Mapped { flags, .. }
                    if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                        && (!writable || flags.contains(PageTableFlags::WRITABLE)) => {}
                _ => return Err(AddressSpaceError::InvalidCopyRange),
            }
            address = page
                .start_address()
                .as_u64()
                .checked_add(Size4KiB::SIZE)
                .ok_or(AddressSpaceError::InvalidCopyRange)?;
        }
        Ok(())
    }

    fn copy_user_chunks(
        &mut self,
        kernel: &mut KernelAddressSpace,
        range: CopyRange,
        buffer_len: usize,
        source: *const u8,
        destination: *mut u8,
        direction: CopyDirection,
    ) -> Result<(), AddressSpaceError> {
        let mut address = range.start;
        let mut offset = 0usize;
        while address < range.end {
            let page_offset = (address as usize) & (Size4KiB::SIZE as usize - 1);
            let page_remaining = Size4KiB::SIZE as usize - page_offset;
            let remaining = buffer_len - offset;
            let chunk_len = core::cmp::min(page_remaining, remaining);
            let physical = match self.translate(kernel, VirtAddr::new(address))? {
                Translation::Mapped { phys_addr, flags }
                    if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                        && (direction == CopyDirection::FromUser
                            || flags.contains(PageTableFlags::WRITABLE)) =>
                {
                    phys_addr
                }
                _ => return Err(AddressSpaceError::InvalidCopyRange),
            };
            let alias = kernel.physical_alias(physical)?;
            // SAFETY: the preceding translation is for this page chunk,
            // validates effective USER access (and WRITE for ToUser), and
            // physical_alias validates a supervisor-only mapped physical
            // address. The kernel buffer is a valid uniquely borrowed slice.
            unsafe {
                match direction {
                    CopyDirection::FromUser => core::ptr::copy_nonoverlapping(
                        alias.as_ptr::<u8>(),
                        destination.add(offset),
                        chunk_len,
                    ),
                    CopyDirection::ToUser => core::ptr::copy_nonoverlapping(
                        source.add(offset),
                        alias.as_mut_ptr::<u8>(),
                        chunk_len,
                    ),
                }
            }
            offset = offset
                .checked_add(chunk_len)
                .ok_or(AddressSpaceError::InvalidCopyRange)?;
            address = address
                .checked_add(chunk_len as u64)
                .ok_or(AddressSpaceError::InvalidCopyRange)?;
        }
        Ok(())
    }

    pub fn unmap_user_page(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
    ) -> Result<PhysFrame<Size4KiB>, AddressSpaceError> {
        validate_user_address(page.start_address())?;
        let next_count = self
            .mapped_user_pages
            .checked_sub(1)
            .ok_or(AddressSpaceError::AccountingOverflow)?;
        match self.translate(kernel, page.start_address())? {
            Translation::Mapped { flags, .. }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE) => {}
            _ => return Err(AddressSpaceError::UserPageNotMapped),
        }
        let frame =
            kernel.with_inactive_mapper(self, |mapper, _| paging::unmap_page(mapper, page))?;
        self.mapped_user_pages = next_count;
        Ok(frame)
    }

    pub fn translate(
        &mut self,
        kernel: &mut KernelAddressSpace,
        address: VirtAddr,
    ) -> Result<Translation, AddressSpaceError> {
        if VirtAddr::try_new(address.as_u64()).is_err() {
            return Err(AddressSpaceError::Mapping(MemoryError::NonCanonicalAddress));
        }
        kernel.with_inactive_mapper(self, |mapper, _| Ok(paging::translate(mapper, address)))
    }

    /// Returns the supervisor physical-map alias for a mapped user page.
    /// This narrow helper exists only for the deterministic preemption test,
    /// whose trusted payload increments a private counter page.
    #[cfg(feature = "process-preemption-test")]
    pub(crate) fn test_user_page_kernel_alias(
        &mut self,
        kernel: &mut KernelAddressSpace,
        page: Page<Size4KiB>,
    ) -> Result<VirtAddr, AddressSpaceError> {
        validate_user_address(page.start_address())?;
        match self.translate(kernel, page.start_address())? {
            Translation::Mapped { phys_addr, flags }
                if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                    && flags.contains(PageTableFlags::WRITABLE) =>
            {
                kernel.physical_alias(phys_addr)
            }
            _ => Err(AddressSpaceError::UserPageNotMapped),
        }
    }
}

#[derive(Clone, Copy)]
struct CopyRange {
    start: u64,
    end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CopyDirection {
    FromUser,
    ToUser,
}

fn validate_copy_range(start: u64, len: usize) -> Result<CopyRange, AddressSpaceError> {
    if len == 0 {
        return Ok(CopyRange { start, end: start });
    }
    let end = start
        .checked_add(u64::try_from(len).map_err(|_| AddressSpaceError::InvalidCopyRange)?)
        .ok_or(AddressSpaceError::InvalidCopyRange)?;
    if VirtAddr::try_new(start).is_err()
        || VirtAddr::try_new(end - 1).is_err()
        || start < super::USER_MIN
        || end > super::USER_MAX
    {
        return Err(AddressSpaceError::InvalidCopyRange);
    }
    Ok(CopyRange { start, end })
}

fn validate_user_address(address: VirtAddr) -> Result<(), AddressSpaceError> {
    if address.as_u64() < super::USER_MIN || address.as_u64() >= super::USER_MAX {
        return Err(AddressSpaceError::OutsideUserRange);
    }
    Ok(())
}

fn validate_user_page(
    page: Page<Size4KiB>,
    permissions: Permissions,
) -> Result<(), AddressSpaceError> {
    validate_user_address(page.start_address())?;
    let end = page
        .start_address()
        .as_u64()
        .checked_add(Size4KiB::SIZE)
        .ok_or(AddressSpaceError::OutsideUserRange)?;
    if end > super::USER_MAX {
        return Err(AddressSpaceError::OutsideUserRange);
    }
    if !permissions.user_accessible {
        return Err(AddressSpaceError::NonUserPermissions);
    }
    if permissions.writable && permissions.executable {
        return Err(AddressSpaceError::Mapping(MemoryError::InvalidPermissions));
    }
    Ok(())
}

/// The kernel's active page-table mapper plus the boot-time physical frame
/// allocator, held behind one lock so the two can never fall out of sync
/// with each other.
pub struct KernelAddressSpace {
    mapper: OffsetPageTable<'static>,
    frame_allocator: BootFrameAllocator,
    physical_memory_offset: VirtAddr,
    boot_mappings: BootMappingInventory,
    kernel_root: PhysFrame<Size4KiB>,
    kernel_root_flags: Cr3Flags,
    boot_mapping_audited: bool,
}

impl KernelAddressSpace {
    /// Loads a process root only after proving that its kernel-half PML4
    /// entries exactly match the currently audited kernel root. This keeps
    /// the active kernel stack, switch code, globals, GDT, IDT and TSS mapped
    /// at identical supervisor-only VAs across the load.
    pub fn activate_address_space(
        &mut self,
        address_space: &AddressSpace,
    ) -> Result<(), AddressSpaceError> {
        self.activate_handle(address_space.handle())
    }

    /// Loads a previously issued opaque process-root handle after validating
    /// that it is still represented by the audited shared kernel-half tree.
    pub(crate) fn activate_handle(
        &mut self,
        address_space: AddressSpaceHandle,
    ) -> Result<(), AddressSpaceError> {
        if x86_64::instructions::interrupts::are_enabled() {
            return Err(AddressSpaceError::UnsafeKernelMapping);
        }
        if !self.boot_mapping_audited {
            return Err(AddressSpaceError::UnsafeKernelMapping);
        }
        self.verify_kernel_clone(address_space)?;
        // SAFETY: the root is owned by AddressSpace, four-level paging was
        // validated at boot, the exact supervisor kernel PML4 mappings were
        // compared immediately above, and IF is clear through the call.
        unsafe { Cr3::write(address_space.0, self.kernel_root_flags) };
        Ok(())
    }

    /// Restores the audited kernel root. Used on the return-to-kernel side of
    /// a switch plan and by root-switch validation tests.
    pub fn activate_kernel_root(&mut self) -> Result<(), AddressSpaceError> {
        if x86_64::instructions::interrupts::are_enabled() {
            return Err(AddressSpaceError::UnsafeKernelMapping);
        }
        // SAFETY: kernel_root is the root captured from CR3 at validated
        // initialization; its mapping tree is audited, and IF is clear.
        unsafe { Cr3::write(self.kernel_root, self.kernel_root_flags) };
        Ok(())
    }

    fn verify_kernel_clone(
        &self,
        address_space: AddressSpaceHandle,
    ) -> Result<(), AddressSpaceError> {
        let alias = self.physical_alias(address_space.0.start_address())?;
        // SAFETY: the root frame is privately owned by this AddressSpace,
        // mapped through the audited supervisor physical map, and read-only
        // for this verification while the kernel address-space lock is held.
        let process_root = unsafe { &*alias.as_ptr::<PageTable>() };
        let kernel_root = self.mapper.level_4_table();
        for index in 256..512 {
            let expected = &kernel_root[index];
            let actual = &process_root[index];
            let expected_present = expected.flags().contains(PageTableFlags::PRESENT);
            let actual_present = actual.flags().contains(PageTableFlags::PRESENT);
            let hardware_flags = !PageTableFlags::ACCESSED;
            if expected_present != actual_present
                || expected_present
                    && (expected.addr() != actual.addr()
                        // Accessed is hardware-managed independently in each
                        // root even when the two entries name the same tree.
                        || expected.flags() & hardware_flags
                            != actual.flags() & hardware_flags
                        || actual.flags().contains(PageTableFlags::USER_ACCESSIBLE))
            {
                return Err(AddressSpaceError::UnsafeKernelMapping);
            }
        }
        Ok(())
    }

    /// Creates an inactive address-space root by cloning the already-audited
    /// supervisor-only upper half. Lower-half bootloader mappings are never
    /// copied into a process root.
    pub fn create_address_space(&mut self) -> Result<AddressSpace, AddressSpaceError> {
        if !self.boot_mapping_audited {
            return Err(AddressSpaceError::UnsafeKernelMapping);
        }
        self.verify_live_kernel_mappings()
            .map_err(|_| AddressSpaceError::KernelMappingMissing)?;

        let root_frame = self
            .frame_allocator
            .allocate_frame()
            .ok_or(AddressSpaceError::FrameAllocationFailed)?;
        let root_alias = self.physical_alias(root_frame.start_address())?;
        // SAFETY: the allocated frame is uniquely owned by the monotonic
        // allocator, its complete page is reachable through the audited
        // supervisor physical map, and no other reference to this new root
        // exists. The closure is IRQ-masked by with_kernel_address_space.
        unsafe {
            core::ptr::write_bytes(root_alias.as_mut_ptr::<u8>(), 0, Size4KiB::SIZE as usize)
        };

        let kernel_root_alias = self.physical_alias(self.kernel_root.start_address())?;
        // SAFETY: kernel_root is the active root captured at initialization;
        // physical_alias proved its mapped supervisor alias. This immutable
        // reference is used only while the address-space lock is held.
        let kernel_root = unsafe { &*kernel_root_alias.as_ptr::<PageTable>() };
        // SAFETY: root_frame is fresh, zeroed, and not active; it has no other
        // references. The mutable reference is scoped to this operation.
        let process_root = unsafe { &mut *root_alias.as_mut_ptr::<PageTable>() };
        for index in 256..512 {
            let entry = &kernel_root[index];
            if !entry.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            if entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                return Err(AddressSpaceError::UnsafeKernelMapping);
            }
            process_root[index] = entry.clone();
        }
        Ok(AddressSpace {
            root: root_frame,
            mapped_user_pages: 0,
        })
    }

    pub(super) fn physical_alias(&self, physical: PhysAddr) -> Result<VirtAddr, AddressSpaceError> {
        let virtual_address = self
            .physical_memory_offset
            .as_u64()
            .checked_add(physical.as_u64())
            .ok_or(AddressSpaceError::InvalidRoot)?;
        let alias =
            VirtAddr::try_new(virtual_address).map_err(|_| AddressSpaceError::InvalidRoot)?;
        self.verify_kernel_root_mapping(alias)?;
        Ok(alias)
    }

    pub(super) fn verify_kernel_root_mapping(
        &self,
        address: VirtAddr,
    ) -> Result<(), AddressSpaceError> {
        match paging::translate(&self.mapper, address) {
            Translation::Mapped { flags, .. }
                if !flags.contains(PageTableFlags::USER_ACCESSIBLE) =>
            {
                Ok(())
            }
            // The audited bootloader physical map may use 2 MiB/1 GiB leaf
            // mappings. Its upper-half tree was recursively checked for
            // USER bits before this alias is accepted.
            Translation::HugePage { .. } => Ok(()),
            _ => Err(AddressSpaceError::UnsafeKernelMapping),
        }
    }

    fn allocate_zeroed_frame(&mut self) -> Result<PhysFrame<Size4KiB>, AddressSpaceError> {
        let frame = self
            .frame_allocator
            .allocate_frame()
            .ok_or(AddressSpaceError::FrameAllocationFailed)?;
        let alias = self.physical_alias(frame.start_address())?;
        // SAFETY: the monotonic allocator uniquely returned this full frame;
        // its physical-map alias was verified supervisor-mapped and no user
        // mapping exposes it before initialization completes.
        unsafe { core::ptr::write_bytes(alias.as_mut_ptr::<u8>(), 0, Size4KiB::SIZE as usize) };
        Ok(frame)
    }

    fn with_inactive_mapper<R>(
        &mut self,
        address_space: &mut AddressSpace,
        f: impl FnOnce(&mut OffsetPageTable<'_>, &mut BootFrameAllocator) -> Result<R, MemoryError>,
    ) -> Result<R, AddressSpaceError> {
        let alias = self.physical_alias(address_space.root.start_address())?;
        // SAFETY: this AddressSpace exclusively owns its root frame, which
        // is not loaded in CR3. The kernel address-space mutex serializes all
        // page-table editing and interrupts are disabled, so this is the only
        // mutable root reference and all child frames remain allocator-owned.
        let root = unsafe { &mut *alias.as_mut_ptr::<PageTable>() };
        // SAFETY: physical_memory_offset is the bootloader mapping audited at
        // init and retained for kernel lifetime; `root` is the inactive root.
        let mut mapper = unsafe { OffsetPageTable::new(root, self.physical_memory_offset) };
        f(&mut mapper, &mut self.frame_allocator).map_err(AddressSpaceError::from)
    }

    fn verify_live_kernel_mappings(&self) -> Result<(), &'static str> {
        self.audit_upper_pml4()?;
        let heap_end = super::heap::HEAP_END;
        let mut address = super::heap::HEAP_START;
        while address < heap_end {
            self.verify_kernel_rw_nx(address)?;
            address = address
                .checked_add(Size4KiB::SIZE)
                .ok_or("heap range overflow")?;
        }
        for slot in 0..crate::arch::x86_64::task::MAX_KERNEL_TASKS {
            let (low, high) =
                crate::arch::x86_64::task::stack_bounds(slot).ok_or("invalid task stack slot")?;
            let mut address = low;
            while address < high {
                self.verify_kernel_rw_nx(address)?;
                address = address
                    .checked_add(Size4KiB::SIZE)
                    .ok_or("task stack range overflow")?;
            }
            for guard in [
                low.checked_sub(Size4KiB::SIZE)
                    .ok_or("task stack guard underflow")?,
                high,
            ] {
                if !matches!(
                    paging::translate(&self.mapper, VirtAddr::new(guard)),
                    Translation::NotMapped
                ) {
                    return Err("task stack guard unexpectedly mapped");
                }
            }
        }
        Ok(())
    }

    fn verify_kernel_rw_nx(&self, address: u64) -> Result<(), &'static str> {
        match paging::translate(&self.mapper, VirtAddr::new(address)) {
            Translation::Mapped { flags, .. }
                if flags.contains(PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE)
                    && !flags.contains(PageTableFlags::USER_ACCESSIBLE) =>
            {
                Ok(())
            }
            _ => Err("kernel heap/task stack is not supervisor RW/NX"),
        }
    }

    fn verify_boot_mapping_policy(&self) -> Result<(), &'static str> {
        let required = [
            ("kernel stack", Some(self.boot_mappings.kernel_stack)),
            ("BootInfo", Some(self.boot_mappings.boot_info)),
            ("memory regions", Some(self.boot_mappings.memory_regions)),
            ("physical map", Some(self.boot_mappings.physical_memory)),
            ("framebuffer", self.boot_mappings.framebuffer),
            ("ramdisk", self.boot_mappings.ramdisk),
        ];

        for (name, range) in required {
            let Some(range) = range else { continue };
            self.verify_dynamic_mapping(name, range)?;
        }

        self.verify_kernel_image_segments()?;

        let current_rsp: u64;
        // SAFETY: reads the current stack pointer without changing it.
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) current_rsp, options(nomem, nostack, preserves_flags));
        }
        self.verify_upper_address("current kernel RSP", current_rsp)?;

        let (root_frame, _) = Cr3::read();
        let root_virtual = self.physical_memory_offset + root_frame.start_address().as_u64();
        self.verify_upper_address("active PML4 physical-map alias", root_virtual.as_u64())?;
        self.verify_supervisor_address(root_virtual.as_u64())?;

        self.audit_upper_pml4()?;
        Ok(())
    }

    fn verify_kernel_image_segments(&self) -> Result<(), &'static str> {
        let base = self.boot_mappings.kernel_image_base;
        self.verify_upper_address("kernel image base", base)?;
        if base < super::BOOTLOADER_DYNAMIC_START || base >= super::BOOTLOADER_DYNAMIC_END {
            return Err("kernel image base lies outside the bootloader dynamic arena");
        }

        let elf_end = self
            .boot_mappings
            .kernel_elf_physical
            .checked_add(self.boot_mappings.kernel_elf_len)
            .ok_or("kernel ELF physical range overflow")?;
        if elf_end > self.boot_mappings.physical_memory.len {
            return Err("kernel ELF bytes are outside the physical-memory mapping");
        }
        let elf_virtual = self.physical_memory_offset + self.boot_mappings.kernel_elf_physical;
        self.verify_supervisor_address(elf_virtual.as_u64())?;

        // SAFETY: the bootloader's physical map is audited before this call;
        // checked arithmetic above proves the entire kernel ELF byte range is
        // within the mapped physical extent. This creates a read-only slice
        // over the bootloader-owned image for bounded field decoding only.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                elf_virtual.as_ptr::<u8>(),
                usize::try_from(self.boot_mappings.kernel_elf_len)
                    .map_err(|_| "kernel ELF length does not fit usize")?,
            )
        };
        if bytes.get(0..4) != Some(b"\x7fELF")
            || bytes.get(4) != Some(&2)
            || bytes.get(5) != Some(&1)
            || read_u16(bytes, 18)? != 62
            || read_u16(bytes, 16)? != 3
        {
            return Err("bootloader kernel ELF identity is unsupported");
        }
        let entry = read_u64(bytes, 24)?;
        let phoff = usize::try_from(read_u64(bytes, 32)?)
            .map_err(|_| "kernel program-header offset does not fit usize")?;
        let phentsize = usize::from(read_u16(bytes, 54)?);
        let phnum = usize::from(read_u16(bytes, 56)?);
        if phentsize != 56 {
            return Err("kernel program-header entry size is invalid");
        }
        let table_len = phentsize
            .checked_mul(phnum)
            .ok_or("kernel program-header table size overflow")?;
        let table_end = phoff
            .checked_add(table_len)
            .ok_or("kernel program-header table range overflow")?;
        if table_end > bytes.len() {
            return Err("kernel program-header table is truncated");
        }

        let entry_virtual = base.checked_add(entry).ok_or("kernel entry overflow")?;
        self.verify_upper_address("kernel entry", entry_virtual)?;
        let mut entry_in_executable_segment = false;
        let mut load_segments = 0u16;
        for index in 0..phnum {
            let header = phoff
                .checked_add(
                    index
                        .checked_mul(phentsize)
                        .ok_or("program-header offset overflow")?,
                )
                .ok_or("program-header address overflow")?;
            if read_u32(bytes, header)? != 1 {
                continue;
            }
            let flags = read_u32(bytes, header + 4)?;
            let writable = flags & 2 != 0;
            let executable = flags & 1 != 0;
            if writable && executable {
                return Err("bootloader kernel PT_LOAD violates W^X");
            }
            let vaddr = read_u64(bytes, header + 16)?;
            let memsz = read_u64(bytes, header + 40)?;
            if memsz == 0 {
                continue;
            }
            let start = base
                .checked_add(vaddr)
                .ok_or("kernel segment VA overflow")?;
            let end = start
                .checked_add(memsz)
                .ok_or("kernel segment end overflow")?;
            self.verify_upper_address("kernel PT_LOAD start", start)?;
            self.verify_upper_address("kernel PT_LOAD end", end - 1)?;
            if start < super::BOOTLOADER_DYNAMIC_START || end > super::BOOTLOADER_DYNAMIC_END {
                return Err("kernel PT_LOAD segment lies outside the dynamic arena");
            }
            if flags & 1 != 0 && entry_virtual >= start && entry_virtual < end {
                entry_in_executable_segment = true;
            }
            let first_page = start & !(Size4KiB::SIZE - 1);
            let rounded_end = end
                .checked_add(Size4KiB::SIZE - 1)
                .ok_or("kernel segment page-rounding overflow")?
                & !(Size4KiB::SIZE - 1);
            let mut page = first_page;
            while page < rounded_end {
                let relro = kernel_page_is_relro(bytes, phoff, phentsize, phnum, base, page)?;
                self.verify_kernel_segment_page(page, writable && !relro, executable)?;
                page = page
                    .checked_add(Size4KiB::SIZE)
                    .ok_or("kernel segment page iteration overflow")?;
            }
            load_segments = load_segments
                .checked_add(1)
                .ok_or("kernel PT_LOAD count overflow")?;
            crate::kprintln!(
                "[memory] audited kernel PT_LOAD: {start:#x}..{end:#x} PML4 {}..{} flags={flags:#x}",
                pml4_index(start),
                pml4_index(end - 1)
            );
        }
        if load_segments == 0 || !entry_in_executable_segment {
            return Err("kernel ELF has no mapped executable entry segment");
        }
        crate::kprintln!(
            "[memory] kernel ELF entry {entry_virtual:#x}; {load_segments} PT_LOAD segments audited"
        );
        Ok(())
    }

    fn verify_kernel_segment_page(
        &self,
        address: u64,
        writable: bool,
        executable: bool,
    ) -> Result<(), &'static str> {
        match paging::translate(&self.mapper, VirtAddr::new(address)) {
            Translation::Mapped { flags, .. } => {
                if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                    || flags.contains(PageTableFlags::WRITABLE) != writable
                    || flags.contains(PageTableFlags::NO_EXECUTE) == executable
                {
                    crate::kprintln!(
                        "[memory] kernel PT_LOAD permission mismatch at {address:#x}: {flags:?}"
                    );
                    return Err("kernel PT_LOAD permissions do not match ELF flags");
                }
                if flags.contains(PageTableFlags::WRITABLE)
                    && !flags.contains(PageTableFlags::NO_EXECUTE)
                {
                    return Err("kernel PT_LOAD mapping violates W^X");
                }
                Ok(())
            }
            Translation::HugePage { .. } => {
                Err("kernel image PT_LOAD unexpectedly uses a huge page")
            }
            Translation::NotMapped | Translation::Unsupported => {
                Err("kernel PT_LOAD page is absent or unsupported")
            }
        }
    }

    fn verify_dynamic_mapping(&self, name: &str, range: MappingRange) -> Result<(), &'static str> {
        if range.len == 0 {
            crate::kprintln!("[memory] required boot mapping has zero length: {name}");
            return Err("required boot mapping has zero length");
        }
        let end = range
            .start
            .checked_add(range.len)
            .ok_or("required boot mapping range overflow")?;
        self.verify_upper_address(name, range.start)?;
        self.verify_upper_address(name, end - 1)?;
        if range.start < super::BOOTLOADER_DYNAMIC_START || end > super::BOOTLOADER_DYNAMIC_END {
            crate::kprintln!(
                "[memory] boot mapping outside dynamic arena: {name} {:#x}..{:#x}",
                range.start,
                end
            );
            return Err("required boot mapping lies outside the reserved dynamic arena");
        }
        if range.start < super::USER_MAX && end > super::USER_MIN {
            crate::kprintln!(
                "[memory] boot mapping overlaps userspace VA policy: {name} {:#x}..{:#x}",
                range.start,
                end
            );
            return Err("required boot mapping overlaps userspace");
        }

        let audit_stride = if name == "physical map" {
            2 * 1024 * 1024
        } else {
            Size4KiB::SIZE
        };
        let mut address = range.start;
        while address < end {
            self.verify_supervisor_address(address)?;
            address = address
                .checked_add(audit_stride)
                .ok_or("required mapping audit address overflow")?;
        }
        self.verify_supervisor_address(end - 1)?;
        crate::kprintln!(
            "[memory] audited {name}: {:#x}..{:#x} (PML4 {}..{})",
            range.start,
            end,
            pml4_index(range.start),
            pml4_index(end - 1)
        );
        Ok(())
    }

    fn verify_upper_address(&self, name: &str, address: u64) -> Result<(), &'static str> {
        if VirtAddr::try_new(address).is_err() || pml4_index(address) < 256 {
            crate::kprintln!(
                "[memory] required mapping is not canonical upper-half: {name} {address:#x}"
            );
            return Err("required kernel mapping is not in the upper canonical half");
        }
        Ok(())
    }

    fn verify_supervisor_address(&self, address: u64) -> Result<(), &'static str> {
        match paging::translate(
            &self.mapper,
            VirtAddr::try_new(address).map_err(|_| "noncanonical mapping")?,
        ) {
            Translation::Mapped { flags, .. } => {
                if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                    crate::kprintln!(
                        "[memory] required kernel mapping is USER at {address:#x}: {flags:?}"
                    );
                    Err("required kernel mapping is user-accessible")
                } else {
                    Ok(())
                }
            }
            Translation::HugePage { .. } => Ok(()),
            Translation::NotMapped | Translation::Unsupported => {
                crate::kprintln!("[memory] required kernel mapping missing at {address:#x}");
                Err("required kernel mapping is absent")
            }
        }
    }

    fn audit_upper_pml4(&self) -> Result<(), &'static str> {
        let root = self.mapper.level_4_table();
        for index in 256..512 {
            let entry = &root[index];
            let flags = entry.flags();
            if !flags.contains(PageTableFlags::PRESENT) {
                continue;
            }
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                crate::kprintln!("[memory] USER bit set in kernel PML4 entry {index}");
                return Err("upper-half PML4 entry is user-accessible");
            }
            self.audit_table(entry.addr(), 3)?;
        }
        Ok(())
    }

    fn audit_table(&self, frame: PhysAddr, level: u8) -> Result<(), &'static str> {
        let virtual_address = self.physical_memory_offset + frame.as_u64();
        self.verify_supervisor_address(virtual_address.as_u64())?;
        // SAFETY: `frame` is reached through the verified supervisor-only
        // physical map; this walk is read-only and IRQs are disabled while
        // the single kernel address-space lock is held during initialization.
        let table = unsafe { &*virtual_address.as_ptr::<PageTable>() };
        for entry in table.iter() {
            let flags = entry.flags();
            if !flags.contains(PageTableFlags::PRESENT) {
                continue;
            }
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                return Err("upper-half page-table entry is user-accessible");
            }
            if level > 1 && !flags.contains(PageTableFlags::HUGE_PAGE) {
                self.audit_table(entry.addr(), level - 1)?;
            }
        }
        Ok(())
    }

    /// Allocates a fresh physical frame and maps `page` to it with
    /// `permissions`. Returns the frame that was mapped.
    pub fn map_new_page(
        &mut self,
        page: Page<Size4KiB>,
        permissions: Permissions,
    ) -> Result<PhysFrame<Size4KiB>, MemoryError> {
        if permissions.user_accessible {
            let start = page.start_address().as_u64();
            let end = start
                .checked_add(Size4KiB::SIZE)
                .ok_or(MemoryError::NonCanonicalAddress)?;
            if start < super::USER_MIN
                || end > super::USER_MAX
                || permissions.writable && permissions.executable
            {
                return Err(MemoryError::InvalidPermissions);
            }
        }
        let frame = self
            .frame_allocator
            .allocate_frame()
            .ok_or(MemoryError::FrameAllocationFailed)?;
        paging::map_page(
            &mut self.mapper,
            &mut self.frame_allocator,
            page,
            frame,
            permissions,
        )?;
        Ok(frame)
    }

    /// Unmaps `page` and flushes its TLB entry. See
    /// [`paging::unmap_page`] for why the returned frame cannot be reused.
    pub fn unmap_page(&mut self, page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, MemoryError> {
        paging::unmap_page(&mut self.mapper, page)
    }

    /// Changes the permissions of an existing mapping and flushes its TLB
    /// entry.
    pub fn update_page_permissions(
        &mut self,
        page: Page<Size4KiB>,
        permissions: Permissions,
    ) -> Result<(), MemoryError> {
        if permissions.user_accessible {
            let start = page.start_address().as_u64();
            let end = start
                .checked_add(Size4KiB::SIZE)
                .ok_or(MemoryError::NonCanonicalAddress)?;
            if start < super::USER_MIN
                || end > super::USER_MAX
                || permissions.writable && permissions.executable
            {
                return Err(MemoryError::InvalidPermissions);
            }
        }
        paging::update_permissions(&mut self.mapper, page, permissions)
    }

    /// Translates a virtual address to its current mapping state.
    pub fn translate(&self, addr: VirtAddr) -> Translation {
        paging::translate(&self.mapper, addr)
    }

    /// Total number of 4 KiB frames the boot frame allocator could ever
    /// hand out. See [`BootFrameAllocator`] for why this is only a boot-time
    /// figure, not a live free-memory count.
    pub fn usable_frame_count(&self) -> usize {
        self.frame_allocator.usable_frame_count()
    }

    /// Number of frames the boot frame allocator has handed out so far.
    pub fn allocated_frame_count(&self) -> usize {
        self.frame_allocator.allocated_frame_count()
    }
}

/// Returns a mutable reference to the currently active level-4 page table.
///
/// Callers must not reach this function before
/// `super::validate_paging_prerequisites` has confirmed CR4.LA57 is clear:
/// `Cr3::read()` always returns a frame address regardless of paging mode,
/// but interpreting it as a four-level `PageTable` (as this function and
/// `OffsetPageTable` both do) is only correct in that mode. `super::init`
/// enforces the ordering (validate, then call this function), so by the
/// time this runs, four-level paging is a confirmed precondition, not an
/// assumption.
///
/// # Safety
///
/// The caller must guarantee that:
///
/// - four-level paging is active (CR4.LA57 clear), per the above;
/// - `phys_mem_offset` is the actual virtual address at which the
///   bootloader mapped the whole of physical memory (i.e. the bootloader
///   was configured with `mappings.physical_memory = Some(Mapping::Dynamic)`
///   and `phys_mem_offset` came from `BootInfo::physical_memory_offset`);
/// - the active CR3 frame is therefore reachable at
///   `phys_mem_offset + CR3_physical_address`;
/// - this function is called at most once for the life of the kernel (or,
///   if called again, that no other `&mut PageTable` obtained from a
///   previous call is still alive), so no two mutable references to the
///   same page table ever coexist;
/// - the returned reference does not outlive the physical-memory mapping it
///   depends on (which, once established at boot, lasts for the kernel's
///   entire lifetime).
unsafe fn active_level_4_table(phys_mem_offset: VirtAddr) -> &'static mut PageTable {
    let (level_4_frame, _) = Cr3::read();
    let phys: PhysAddr = level_4_frame.start_address();
    let virt = phys_mem_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    // SAFETY: forwarded from this function's own safety contract above; the
    // pointer was computed from the live CR3 frame and the caller-verified
    // physical-memory offset, so it points at the actual active level-4
    // table, and the single-call invariant means this is the only mutable
    // reference to it.
    unsafe { &mut *page_table_ptr }
}

fn pml4_index(address: u64) -> u16 {
    ((address >> 39) & 0x1ff) as u16
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    let data = bytes
        .get(offset..offset.checked_add(2).ok_or("ELF field offset overflow")?)
        .ok_or("kernel ELF field is truncated")?;
    Ok(u16::from_le_bytes([data[0], data[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    let data = bytes
        .get(offset..offset.checked_add(4).ok_or("ELF field offset overflow")?)
        .ok_or("kernel ELF field is truncated")?;
    Ok(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, &'static str> {
    let data = bytes
        .get(offset..offset.checked_add(8).ok_or("ELF field offset overflow")?)
        .ok_or("kernel ELF field is truncated")?;
    Ok(u64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]))
}

fn kernel_page_is_relro(
    bytes: &[u8],
    phoff: usize,
    phentsize: usize,
    phnum: usize,
    image_base: u64,
    page: u64,
) -> Result<bool, &'static str> {
    const PT_GNU_RELRO: u32 = 0x6474_e552;
    for index in 0..phnum {
        let header = phoff
            .checked_add(
                index
                    .checked_mul(phentsize)
                    .ok_or("program-header offset overflow")?,
            )
            .ok_or("program-header address overflow")?;
        if read_u32(bytes, header)? != PT_GNU_RELRO {
            continue;
        }
        let start = image_base
            .checked_add(read_u64(bytes, header + 16)?)
            .ok_or("kernel RELRO start overflow")?;
        let len = read_u64(bytes, header + 40)?;
        if len == 0 {
            continue;
        }
        let end = start.checked_add(len).ok_or("kernel RELRO end overflow")?;
        let rounded_start = start & !(Size4KiB::SIZE - 1);
        let rounded_end = end
            .checked_add(Size4KiB::SIZE - 1)
            .ok_or("kernel RELRO page-rounding overflow")?
            & !(Size4KiB::SIZE - 1);
        if page >= rounded_start && page < rounded_end {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Builds the single kernel address space. Must be called at most once; see
/// [`super::init`], which enforces this through [`Once::call_once`].
pub(super) fn init(
    phys_mem_offset: VirtAddr,
    frame_allocator: BootFrameAllocator,
    boot_mappings: BootMappingInventory,
) {
    KERNEL_ADDRESS_SPACE.call_once(|| {
        // SAFETY: `phys_mem_offset` is forwarded unchanged from
        // `BootInfo::physical_memory_offset` by `super::init`, which itself
        // only runs this closure once via `Once::call_once`, so no
        // competing mutable reference to the level-4 table is created
        // elsewhere.
        let level_4_table = unsafe { active_level_4_table(phys_mem_offset) };

        // SAFETY: `level_4_table` was just obtained above and is not
        // referenced anywhere else; `phys_mem_offset` is the same value
        // used to locate it, which is exactly what `OffsetPageTable::new`
        // requires.
        let mapper = unsafe { OffsetPageTable::new(level_4_table, phys_mem_offset) };

        let (kernel_root, kernel_root_flags) = Cr3::read();
        let mut space = KernelAddressSpace {
            mapper,
            frame_allocator,
            physical_memory_offset: phys_mem_offset,
            boot_mappings,
            kernel_root,
            kernel_root_flags,
            boot_mapping_audited: false,
        };
        space
            .verify_boot_mapping_policy()
            .expect("runtime boot mapping audit failed; refusing process roots");
        space.boot_mapping_audited = true;
        Mutex::new(space)
    });
}

/// Runs `f` with exclusive access to the kernel address space.
///
/// Disables maskable hardware interrupts for the duration of the call. No
/// current interrupt handler touches page-table state, but this keeps that
/// invariant explicit and enforced rather than implicit, and prevents a
/// future handler from ever observing a page table mid-update.
///
/// # Panics
///
/// Panics if [`init`] has not run yet; this indicates a boot-ordering bug
/// (memory subsystem used before `memory::init` ran), not an ordinary
/// runtime condition.
pub fn with_kernel_address_space<R>(f: impl FnOnce(&mut KernelAddressSpace) -> R) -> R {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let lock = KERNEL_ADDRESS_SPACE
            .get()
            .expect("memory subsystem used before memory::init ran");
        let mut guard = lock.lock();
        f(&mut guard)
    })
}
