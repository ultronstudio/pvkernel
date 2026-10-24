//! Ownership of the active x86_64 page table and the boot frame allocator.
//!
//! There is exactly one [`KernelAddressSpace`] for the whole kernel in this
//! milestone (pvKernel 0.0.5 does not create per-process address spaces).
//! It is built once during [`super::init`] and afterward is only reachable
//! through [`with_kernel_address_space`], which serializes access behind a
//! [`spin::Mutex`] and disables maskable interrupts for the duration of the
//! call. This keeps a single owner of the mapper at all times: no code path
//! can obtain two simultaneous mutable references to the page tables.

use spin::{Mutex, Once};
use x86_64::{
    PhysAddr, VirtAddr,
    registers::control::Cr3,
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

/// The kernel's active page-table mapper plus the boot-time physical frame
/// allocator, held behind one lock so the two can never fall out of sync
/// with each other.
pub struct KernelAddressSpace {
    mapper: OffsetPageTable<'static>,
    frame_allocator: BootFrameAllocator,
    physical_memory_offset: VirtAddr,
    boot_mappings: BootMappingInventory,
}

impl KernelAddressSpace {
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

        let space = KernelAddressSpace {
            mapper,
            frame_allocator,
            physical_memory_offset: phys_mem_offset,
            boot_mappings,
        };
        space
            .verify_boot_mapping_policy()
            .expect("runtime boot mapping audit failed; refusing process roots");
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
