//! Bounded checked loader for static x86_64 ELF64 `ET_EXEC` images.

#[cfg(feature = "qemu-test")]
use x86_64::structures::paging::PageTableFlags;
use x86_64::{
    VirtAddr,
    structures::paging::{Page, PageSize, Size4KiB},
};

use crate::arch::x86_64::memory::{
    AddressSpace, AddressSpaceError, KernelAddressSpace, Permissions, Translation,
};

const ELF_MIN: u64 = 0x0000_0000_0040_0000;
const ELF_MAX: u64 = 0x0000_4000_0000_0000;
const MAX_LOAD_SEGMENTS: usize = 16;
const MAX_IMAGE_PAGES: u64 = 4096;
const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_TLS: u32 = 7;
const PF_R: u32 = 4;
const PF_W: u32 = 2;
const PF_X: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    Truncated,
    BadMagic,
    UnsupportedClass,
    UnsupportedEndian,
    UnsupportedVersion,
    UnsupportedMachine,
    UnsupportedType,
    InvalidHeaderSize,
    InvalidProgramHeaderSize,
    InvalidProgramHeaderTable,
    UnsupportedInterpreter,
    UnsupportedDynamicImage,
    UnsupportedTls,
    TooManySegments,
    TooManyPages,
    InvalidSegment,
    FileRangeOverflow,
    VirtualRangeOverflow,
    OutsideUserRange,
    NonCanonicalAddress,
    InvalidAlignment,
    SegmentOverlap,
    StackOverlap,
    WritableExecutable,
    InvalidEntry,
    EntryNotExecutable,
    SegmentAlreadyMapped,
    AddressSpace(AddressSpaceError),
}

impl From<AddressSpaceError> for ElfError {
    fn from(error: AddressSpaceError) -> Self {
        Self::AddressSpace(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Segment {
    file_offset: usize,
    file_size: usize,
    virtual_start: u64,
    memory_end: u64,
    page_start: u64,
    page_end: u64,
    flags: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ElfPlan {
    entry: u64,
    segments: [Option<Segment>; MAX_LOAD_SEGMENTS],
    segment_count: usize,
    page_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedImage {
    pub entry: u64,
    pub mapped_pages: u64,
}

/// Validates all headers and load ranges before allocating or mapping any
/// user frame. The image is static ET_EXEC: there is no dynamic linker,
/// relocation, TLS, PIE, or demand paging.
fn validate(bytes: &[u8]) -> Result<ElfPlan, ElfError> {
    if bytes.len() < ELF_HEADER_SIZE {
        return Err(ElfError::Truncated);
    }
    if bytes.get(0..4) != Some(b"\x7fELF") {
        return Err(ElfError::BadMagic);
    }
    if bytes[4] != 2 {
        return Err(ElfError::UnsupportedClass);
    }
    if bytes[5] != 1 {
        return Err(ElfError::UnsupportedEndian);
    }
    if bytes[6] != 1 {
        return Err(ElfError::UnsupportedVersion);
    }
    if read_u16(bytes, 18)? != 62 {
        return Err(ElfError::UnsupportedMachine);
    }
    if read_u16(bytes, 16)? != 2 {
        return Err(ElfError::UnsupportedType);
    }
    if read_u32(bytes, 20)? != 1 || read_u16(bytes, 52)? as usize != ELF_HEADER_SIZE {
        return Err(ElfError::InvalidHeaderSize);
    }
    let entry = read_u64(bytes, 24)?;
    let phoff =
        usize::try_from(read_u64(bytes, 32)?).map_err(|_| ElfError::InvalidProgramHeaderTable)?;
    let phentsize = read_u16(bytes, 54)? as usize;
    let phnum = read_u16(bytes, 56)? as usize;
    if phentsize != PROGRAM_HEADER_SIZE {
        return Err(ElfError::InvalidProgramHeaderSize);
    }
    if phnum == 0 {
        return Err(ElfError::InvalidProgramHeaderTable);
    }
    let table_size = phentsize
        .checked_mul(phnum)
        .ok_or(ElfError::InvalidProgramHeaderTable)?;
    let table_end = phoff
        .checked_add(table_size)
        .ok_or(ElfError::InvalidProgramHeaderTable)?;
    if phoff < ELF_HEADER_SIZE || table_end > bytes.len() {
        return Err(ElfError::InvalidProgramHeaderTable);
    }

    let mut plan = ElfPlan {
        entry,
        segments: [None; MAX_LOAD_SEGMENTS],
        segment_count: 0,
        page_count: 0,
    };
    let mut entry_executable = false;
    for index in 0..phnum {
        let offset = phoff
            .checked_add(
                index
                    .checked_mul(phentsize)
                    .ok_or(ElfError::InvalidProgramHeaderTable)?,
            )
            .ok_or(ElfError::InvalidProgramHeaderTable)?;
        let kind = read_u32(bytes, offset)?;
        if kind == PT_INTERP {
            return Err(ElfError::UnsupportedInterpreter);
        }
        if kind == PT_DYNAMIC {
            return Err(ElfError::UnsupportedDynamicImage);
        }
        if kind == PT_TLS {
            return Err(ElfError::UnsupportedTls);
        }
        if kind != PT_LOAD {
            continue;
        }
        if plan.segment_count == MAX_LOAD_SEGMENTS {
            return Err(ElfError::TooManySegments);
        }

        let flags = read_u32(bytes, offset + 4)?;
        if flags & !(PF_R | PF_W | PF_X) != 0 {
            return Err(ElfError::InvalidSegment);
        }
        let file_offset_u64 = read_u64(bytes, offset + 8)?;
        let virtual_start = read_u64(bytes, offset + 16)?;
        let file_size_u64 = read_u64(bytes, offset + 32)?;
        let memory_size = read_u64(bytes, offset + 40)?;
        let alignment = read_u64(bytes, offset + 48)?;
        if file_size_u64 > memory_size {
            return Err(ElfError::InvalidSegment);
        }
        if flags & PF_W != 0 && flags & PF_X != 0 {
            return Err(ElfError::WritableExecutable);
        }
        if memory_size == 0 {
            if file_size_u64 != 0 {
                return Err(ElfError::InvalidSegment);
            }
            continue;
        }
        let file_offset_end = file_offset_u64
            .checked_add(file_size_u64)
            .ok_or(ElfError::FileRangeOverflow)?;
        if file_offset_end > bytes.len() as u64 {
            return Err(ElfError::InvalidSegment);
        }
        let file_offset = usize::try_from(file_offset_u64).map_err(|_| ElfError::InvalidSegment)?;
        let file_size = usize::try_from(file_size_u64).map_err(|_| ElfError::InvalidSegment)?;
        let memory_end = virtual_start
            .checked_add(memory_size)
            .ok_or(ElfError::VirtualRangeOverflow)?;
        if VirtAddr::try_new(virtual_start).is_err() || VirtAddr::try_new(memory_end - 1).is_err() {
            return Err(ElfError::NonCanonicalAddress);
        }
        if virtual_start < crate::arch::x86_64::memory::USER_MIN {
            return Err(ElfError::OutsideUserRange);
        }
        if alignment > 1 {
            if !alignment.is_power_of_two()
                || virtual_start % alignment != file_offset_u64 % alignment
            {
                return Err(ElfError::InvalidAlignment);
            }
        }
        let page_start = virtual_start & !(Size4KiB::SIZE - 1);
        let page_end = memory_end
            .checked_add(Size4KiB::SIZE - 1)
            .ok_or(ElfError::VirtualRangeOverflow)?
            & !(Size4KiB::SIZE - 1);
        let pages = page_end
            .checked_sub(page_start)
            .ok_or(ElfError::VirtualRangeOverflow)?
            / Size4KiB::SIZE;
        plan.page_count = plan
            .page_count
            .checked_add(pages)
            .ok_or(ElfError::TooManyPages)?;
        if plan.page_count > MAX_IMAGE_PAGES {
            return Err(ElfError::TooManyPages);
        }
        for previous in plan.segments[..plan.segment_count].iter().flatten() {
            if page_start < previous.page_end && page_end > previous.page_start {
                return Err(ElfError::SegmentOverlap);
            }
        }
        let stack_guard = crate::arch::x86_64::memory::USER_STACK_TOP
            .checked_sub(crate::arch::x86_64::memory::USER_STACK_SIZE)
            .and_then(|bottom| bottom.checked_sub(Size4KiB::SIZE))
            .ok_or(ElfError::VirtualRangeOverflow)?;
        if page_start < crate::arch::x86_64::memory::USER_STACK_TOP && page_end > stack_guard {
            return Err(ElfError::StackOverlap);
        }
        if virtual_start < ELF_MIN || memory_end > ELF_MAX {
            return Err(ElfError::OutsideUserRange);
        }
        if flags & PF_X != 0 && entry >= virtual_start && entry < memory_end {
            entry_executable = true;
        }
        plan.segments[plan.segment_count] = Some(Segment {
            file_offset,
            file_size,
            virtual_start,
            memory_end,
            page_start,
            page_end,
            flags,
        });
        plan.segment_count += 1;
    }
    if plan.segment_count == 0 {
        return Err(ElfError::InvalidProgramHeaderTable);
    }
    if VirtAddr::try_new(entry).is_err() {
        return Err(ElfError::NonCanonicalAddress);
    }
    if entry < ELF_MIN || entry >= ELF_MAX {
        return Err(ElfError::InvalidEntry);
    }
    if !entry_executable {
        return Err(ElfError::EntryNotExecutable);
    }
    Ok(plan)
}

/// Loads the validated image into an inactive process root. New user pages
/// are zeroed RW/NX before bytes are copied, then changed to final segment
/// permissions. Any failure removes all image leaves; backing frames remain
/// kernel-lifetime owned by the monotonic allocator.
pub fn load_static_elf(
    address_space: &mut AddressSpace,
    kernel: &mut KernelAddressSpace,
    bytes: &[u8],
) -> Result<LoadedImage, ElfError> {
    let plan = validate(bytes)?;
    for segment in plan.segments[..plan.segment_count].iter().flatten() {
        let mut address = segment.page_start;
        while address < segment.page_end {
            match address_space.translate(kernel, VirtAddr::new(address))? {
                Translation::NotMapped => {}
                _ => return Err(ElfError::SegmentAlreadyMapped),
            }
            address = address
                .checked_add(Size4KiB::SIZE)
                .ok_or(ElfError::VirtualRangeOverflow)?;
        }
    }

    for segment in plan.segments[..plan.segment_count].iter().flatten() {
        let mut address = segment.page_start;
        while address < segment.page_end {
            let page = Page::<Size4KiB>::from_start_address(VirtAddr::new(address))
                .map_err(|_| ElfError::VirtualRangeOverflow)?;
            if let Err(error) =
                address_space.map_new_user_page(kernel, page, Permissions::USER_DATA)
            {
                cleanup(address_space, kernel, &plan);
                return Err(ElfError::AddressSpace(error));
            }
            address = address
                .checked_add(Size4KiB::SIZE)
                .ok_or(ElfError::VirtualRangeOverflow)?;
        }
    }

    for segment in plan.segments[..plan.segment_count].iter().flatten() {
        let mut copied = 0_u64;
        while copied < segment.file_size as u64 {
            let virtual_address = segment
                .virtual_start
                .checked_add(copied)
                .ok_or(ElfError::VirtualRangeOverflow)?;
            let page_start = virtual_address & !(Size4KiB::SIZE - 1);
            let page_offset = usize::try_from(virtual_address - page_start)
                .map_err(|_| ElfError::VirtualRangeOverflow)?;
            let remaining = (segment.file_size as u64)
                .checked_sub(copied)
                .ok_or(ElfError::InvalidSegment)?;
            let chunk = core::cmp::min(remaining, Size4KiB::SIZE - page_offset as u64) as usize;
            let source_start = segment
                .file_offset
                .checked_add(usize::try_from(copied).map_err(|_| ElfError::FileRangeOverflow)?)
                .ok_or(ElfError::FileRangeOverflow)?;
            let source_end = source_start
                .checked_add(chunk)
                .ok_or(ElfError::FileRangeOverflow)?;
            let source = bytes
                .get(source_start..source_end)
                .ok_or(ElfError::InvalidSegment)?;
            let page = Page::<Size4KiB>::from_start_address(VirtAddr::new(page_start))
                .map_err(|_| ElfError::VirtualRangeOverflow)?;
            if let Err(error) = address_space.write_user_page(kernel, page, page_offset, source) {
                cleanup(address_space, kernel, &plan);
                return Err(ElfError::AddressSpace(error));
            }
            copied = copied
                .checked_add(chunk as u64)
                .ok_or(ElfError::FileRangeOverflow)?;
        }
    }

    for segment in plan.segments[..plan.segment_count].iter().flatten() {
        let permissions = if segment.flags & PF_X != 0 {
            Permissions::USER_EXECUTABLE
        } else if segment.flags & PF_W != 0 {
            Permissions::USER_DATA
        } else {
            Permissions::USER_READ_ONLY
        };
        // x86-64 does not provide a per-page read-disable bit: PF_R is
        // advisory in this loader. PF_W controls WRITABLE and PF_X controls
        // execute-disable; W+X was rejected during validation.
        let mut address = segment.page_start;
        while address < segment.page_end {
            let page = Page::<Size4KiB>::from_start_address(VirtAddr::new(address))
                .map_err(|_| ElfError::VirtualRangeOverflow)?;
            if let Err(error) = address_space.update_user_permissions(kernel, page, permissions) {
                cleanup(address_space, kernel, &plan);
                return Err(ElfError::AddressSpace(error));
            }
            address = address
                .checked_add(Size4KiB::SIZE)
                .ok_or(ElfError::VirtualRangeOverflow)?;
        }
    }
    Ok(LoadedImage {
        entry: plan.entry,
        mapped_pages: plan.page_count,
    })
}

fn cleanup(address_space: &mut AddressSpace, kernel: &mut KernelAddressSpace, plan: &ElfPlan) {
    for segment in plan.segments[..plan.segment_count].iter().flatten() {
        let mut address = segment.page_start;
        while address < segment.page_end {
            if let Ok(page) = Page::<Size4KiB>::from_start_address(VirtAddr::new(address)) {
                if !matches!(
                    address_space.translate(kernel, VirtAddr::new(address)),
                    Ok(Translation::NotMapped)
                ) {
                    let _ = address_space.unmap_user_page(kernel, page);
                }
            }
            let Some(next) = address.checked_add(Size4KiB::SIZE) else {
                break;
            };
            address = next;
        }
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    let end = offset.checked_add(2).ok_or(ElfError::Truncated)?;
    let value = bytes.get(offset..end).ok_or(ElfError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    let end = offset.checked_add(4).ok_or(ElfError::Truncated)?;
    let value = bytes.get(offset..end).ok_or(ElfError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    let end = offset.checked_add(8).ok_or(ElfError::Truncated)?;
    let value = bytes.get(offset..end).ok_or(ElfError::Truncated)?;
    Ok(u64::from_le_bytes(
        value.try_into().map_err(|_| ElfError::Truncated)?,
    ))
}

#[cfg(feature = "qemu-test")]
pub fn run_loader_qemu_tests() {
    use crate::arch::x86_64::memory::with_kernel_address_space;

    crate::kprintln!("[process-test] validating and loading static ELF64 image");
    let image_bytes = test_elf();
    with_kernel_address_space(|kernel| {
        let mut address_space = kernel
            .create_address_space()
            .expect("failed to create ELF test root");
        let loaded = load_static_elf(&mut address_space, kernel, &image_bytes)
            .expect("valid static test ELF was rejected");
        assert_eq!(loaded.entry, 0x0040_0000);
        assert_eq!(loaded.mapped_pages, 3);
        assert_eq!(address_space.mapped_user_pages(), 3);

        let code_page = Page::<Size4KiB>::from_start_address(VirtAddr::new(0x0040_0000)).unwrap();
        match address_space
            .translate(kernel, VirtAddr::new(0x0040_0000))
            .unwrap()
        {
            Translation::Mapped { flags, .. } => {
                assert!(flags.contains(PageTableFlags::USER_ACCESSIBLE));
                assert!(!flags.contains(PageTableFlags::WRITABLE));
                assert!(!flags.contains(PageTableFlags::NO_EXECUTE));
            }
            other => panic!("ELF code page missing: {other:?}"),
        }
        let mut code = [0; 8];
        address_space
            .read_user_page(kernel, code_page, 0, &mut code)
            .unwrap();
        assert_eq!(&code[..4], &[0x90, 0x90, 0x90, 0xCC]);
        assert_eq!(&code[4..], &[0; 4], "ELF code page padding leaked data");

        for address in [0x0040_2000, 0x0040_3000] {
            match address_space
                .translate(kernel, VirtAddr::new(address))
                .unwrap()
            {
                Translation::Mapped { flags, .. } => assert!(flags.contains(
                    PageTableFlags::USER_ACCESSIBLE
                        | PageTableFlags::WRITABLE
                        | PageTableFlags::NO_EXECUTE
                )),
                other => panic!("ELF data/BSS page missing: {other:?}"),
            }
        }
        let data_page = Page::<Size4KiB>::from_start_address(VirtAddr::new(0x0040_2000)).unwrap();
        let mut data = [0; 8];
        address_space
            .read_user_page(kernel, data_page, 0, &mut data)
            .unwrap();
        assert_eq!(&data[..4], &[1, 2, 3, 4]);
        assert_eq!(&data[4..], &[0; 4], "ELF BSS tail was not zero");
        let bss_page = Page::<Size4KiB>::from_start_address(VirtAddr::new(0x0040_3000)).unwrap();
        let mut bss = [0xFF; 8];
        address_space
            .read_user_page(kernel, bss_page, 0, &mut bss)
            .unwrap();
        assert_eq!(bss, [0; 8], "ELF BSS page exposed stale contents");
    });

    macro_rules! rejects {
        ($name:literal, $edit:expr, $expected:expr) => {{
            let mut malformed = test_elf();
            ($edit)(&mut malformed);
            assert_eq!(validate(&malformed), Err($expected), $name);
        }};
    }
    rejects!(
        "bad magic",
        |b: &mut alloc::vec::Vec<u8>| b[0] = 0,
        ElfError::BadMagic
    );
    rejects!(
        "ELF32",
        |b: &mut alloc::vec::Vec<u8>| b[4] = 1,
        ElfError::UnsupportedClass
    );
    rejects!(
        "wrong endian",
        |b: &mut alloc::vec::Vec<u8>| b[5] = 2,
        ElfError::UnsupportedEndian
    );
    rejects!(
        "wrong machine",
        |b: &mut alloc::vec::Vec<u8>| put_u16(b, 18, 3),
        ElfError::UnsupportedMachine
    );
    rejects!(
        "unsupported type",
        |b: &mut alloc::vec::Vec<u8>| put_u16(b, 16, 3),
        ElfError::UnsupportedType
    );
    rejects!(
        "PT_INTERP",
        |b: &mut alloc::vec::Vec<u8>| put_u32(b, 64, PT_INTERP),
        ElfError::UnsupportedInterpreter
    );
    rejects!(
        "PT_DYNAMIC",
        |b: &mut alloc::vec::Vec<u8>| put_u32(b, 64, PT_DYNAMIC),
        ElfError::UnsupportedDynamicImage
    );
    rejects!(
        "PT_TLS",
        |b: &mut alloc::vec::Vec<u8>| put_u32(b, 64, PT_TLS),
        ElfError::UnsupportedTls
    );
    rejects!(
        "bad ELF header size",
        |b: &mut alloc::vec::Vec<u8>| put_u16(b, 52, 63),
        ElfError::InvalidHeaderSize
    );
    rejects!(
        "bad program header size",
        |b: &mut alloc::vec::Vec<u8>| put_u16(b, 54, 55),
        ElfError::InvalidProgramHeaderSize
    );
    rejects!(
        "program header table range",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 32, u64::MAX),
        ElfError::InvalidProgramHeaderTable
    );
    rejects!(
        "filesz > memsz",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 120 + 32, 5),
        ElfError::InvalidSegment
    );
    rejects!(
        "offset overflow",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 120 + 8, u64::MAX - 3),
        ElfError::FileRangeOverflow
    );
    rejects!(
        "VA overflow",
        |b: &mut alloc::vec::Vec<u8>| {
            put_u64(b, 120 + 16, u64::MAX - 3);
            put_u64(b, 120 + 40, 8);
        },
        ElfError::VirtualRangeOverflow
    );
    rejects!(
        "segment outside ELF band",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 120 + 16, ELF_MAX),
        ElfError::OutsideUserRange
    );
    rejects!(
        "segment in forbidden low area",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 120 + 16, 0x8000),
        ElfError::OutsideUserRange
    );
    rejects!(
        "segment overlap",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 120 + 16, 0x0040_0000),
        ElfError::SegmentOverlap
    );
    rejects!(
        "stack overlap",
        |b: &mut alloc::vec::Vec<u8>| {
            put_u64(
                b,
                120 + 16,
                crate::arch::x86_64::memory::USER_STACK_TOP - Size4KiB::SIZE,
            );
            put_u64(b, 120 + 40, Size4KiB::SIZE);
        },
        ElfError::StackOverlap
    );
    rejects!(
        "W+X",
        |b: &mut alloc::vec::Vec<u8>| put_u32(b, 64 + 4, PF_W | PF_X),
        ElfError::WritableExecutable
    );
    rejects!(
        "invalid entry",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 24, 0x0040_1000),
        ElfError::EntryNotExecutable
    );
    rejects!(
        "noncanonical segment",
        |b: &mut alloc::vec::Vec<u8>| put_u64(b, 120 + 16, crate::arch::x86_64::memory::USER_MAX),
        ElfError::NonCanonicalAddress
    );
    crate::kprintln!("[process-test] static ELF loader and malformed-header tests passed");
}

#[cfg(feature = "qemu-test")]
fn test_elf() -> alloc::vec::Vec<u8> {
    let mut bytes = alloc::vec![0; 0x2004];
    bytes[..16].copy_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    put_u16(&mut bytes, 16, 2);
    put_u16(&mut bytes, 18, 62);
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, 0x0040_0000);
    put_u64(&mut bytes, 32, ELF_HEADER_SIZE as u64);
    put_u16(&mut bytes, 52, ELF_HEADER_SIZE as u16);
    put_u16(&mut bytes, 54, PROGRAM_HEADER_SIZE as u16);
    put_u16(&mut bytes, 56, 2);
    test_load_header(&mut bytes, 64, PF_X | PF_R, 0x1000, 0x0040_0000, 4, 4);
    test_load_header(&mut bytes, 120, PF_W | PF_R, 0x2000, 0x0040_2000, 4, 0x1004);
    bytes[0x1000..0x1004].copy_from_slice(&[0x90, 0x90, 0x90, 0xCC]);
    bytes[0x2000..0x2004].copy_from_slice(&[1, 2, 3, 4]);
    bytes
}

/// Builds the embedded process-exit smoke ELF with its expected PID baked
/// into the test code. This is a deterministic QEMU fixture, not a loader
/// shortcut or a production executable provider.
pub(crate) fn process_smoke_elf(expected_pid: u64) -> alloc::vec::Vec<u8> {
    const ENTRY: u64 = 0x0040_0000;
    const FILE_OFFSET: usize = 0x1000;
    let mut code = [0u8; 50];
    code[0..5].copy_from_slice(&[0xB8, 3, 0, 0, 0]); // mov eax, GETPID
    code[5..7].copy_from_slice(&[0xCD, 0x80]);
    code[7..9].copy_from_slice(&[0x48, 0xB9]); // mov rcx, expected PID
    code[9..17].copy_from_slice(&expected_pid.to_le_bytes());
    code[17..20].copy_from_slice(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
    code[20..22].copy_from_slice(&[0x75, 0x0E]); // jne failure
    code[22..27].copy_from_slice(&[0xBF, 0, 0, 0, 0]); // mov edi, 0
    code[27..32].copy_from_slice(&[0xB8, 4, 0, 0, 0]); // mov eax, EXIT
    code[32..34].copy_from_slice(&[0xCD, 0x80]);
    code[34..36].copy_from_slice(&[0x0F, 0x0B]); // unreachable UD2 sentinel
    code[36..41].copy_from_slice(&[0xBF, 1, 0, 0, 0]); // mov edi, 1
    code[41..46].copy_from_slice(&[0xB8, 4, 0, 0, 0]);
    code[46..48].copy_from_slice(&[0xCD, 0x80]);
    code[48..50].copy_from_slice(&[0x0F, 0x0B]); // exit must never return

    let mut bytes = alloc::vec![0; FILE_OFFSET + code.len()];
    bytes[..16].copy_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    put_u16(&mut bytes, 16, 2);
    put_u16(&mut bytes, 18, 62);
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, ENTRY);
    put_u64(&mut bytes, 32, ELF_HEADER_SIZE as u64);
    put_u16(&mut bytes, 52, ELF_HEADER_SIZE as u16);
    put_u16(&mut bytes, 54, PROGRAM_HEADER_SIZE as u16);
    put_u16(&mut bytes, 56, 1);
    test_load_header(
        &mut bytes,
        64,
        PF_R | PF_X,
        FILE_OFFSET as u64,
        ENTRY,
        code.len() as u64,
        code.len() as u64,
    );
    bytes[FILE_OFFSET..].copy_from_slice(&code);
    bytes
}

/// Builds one small ELF fixture whose entry triggers a requested hardware
/// exception immediately. Used only to validate CPL3 fault containment.
#[cfg(feature = "user-fault-containment-test")]
pub(crate) fn process_fault_elf(vector: u8) -> alloc::vec::Vec<u8> {
    process_fault_elf_at(vector, 0x3000_0000)
}

#[cfg(feature = "user-fault-containment-test")]
pub(crate) fn process_fault_elf_at(vector: u8, fault_address: u64) -> alloc::vec::Vec<u8> {
    const ENTRY: u64 = 0x0040_0000;
    const FILE_OFFSET: usize = 0x1000;
    let code: &[u8] = match vector {
        6 => &[0x0F, 0x0B],        // UD2
        13 => &[0x0F, 0x20, 0xC0], // MOV RAX, CR0: privileged at CPL3
        14 => return process_page_fault_elf(fault_address),
        _ => panic!("unsupported process fault fixture vector"),
    };

    let mut bytes = alloc::vec![0; FILE_OFFSET + code.len()];
    bytes[..16].copy_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    put_u16(&mut bytes, 16, 2);
    put_u16(&mut bytes, 18, 62);
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, ENTRY);
    put_u64(&mut bytes, 32, ELF_HEADER_SIZE as u64);
    put_u16(&mut bytes, 52, ELF_HEADER_SIZE as u16);
    put_u16(&mut bytes, 54, PROGRAM_HEADER_SIZE as u16);
    put_u16(&mut bytes, 56, 1);
    test_load_header(
        &mut bytes,
        64,
        PF_R | PF_X,
        FILE_OFFSET as u64,
        ENTRY,
        code.len() as u64,
        code.len() as u64,
    );
    bytes[FILE_OFFSET..].copy_from_slice(code);
    bytes
}

#[cfg(feature = "user-fault-containment-test")]
fn process_page_fault_elf(fault_address: u64) -> alloc::vec::Vec<u8> {
    const ENTRY: u64 = 0x0040_0000;
    const FILE_OFFSET: usize = 0x1000;
    let mut code = [0u8; 13];
    code[0..2].copy_from_slice(&[0x48, 0xB8]); // movabs rax, fault_address
    code[2..10].copy_from_slice(&fault_address.to_le_bytes());
    code[10..13].copy_from_slice(&[0x48, 0x8B, 0x00]); // mov rax, [rax]
    let mut bytes = alloc::vec![0; FILE_OFFSET + code.len()];
    bytes[..16].copy_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    put_u16(&mut bytes, 16, 2);
    put_u16(&mut bytes, 18, 62);
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, ENTRY);
    put_u64(&mut bytes, 32, ELF_HEADER_SIZE as u64);
    put_u16(&mut bytes, 52, ELF_HEADER_SIZE as u16);
    put_u16(&mut bytes, 54, PROGRAM_HEADER_SIZE as u16);
    put_u16(&mut bytes, 56, 1);
    test_load_header(
        &mut bytes,
        64,
        PF_R | PF_X,
        FILE_OFFSET as u64,
        ENTRY,
        code.len() as u64,
        code.len() as u64,
    );
    bytes[FILE_OFFSET..].copy_from_slice(&code);
    bytes
}

/// Builds one ELF for a process-isolation test. Both roots map the same
/// private VA, and the payload checks its expected per-process value while
/// repeated PIT/syscall boundaries permit real scheduler switches.
#[cfg(feature = "address-space-isolation-test")]
pub(crate) fn process_isolation_elf(expected: u64) -> alloc::vec::Vec<u8> {
    const ENTRY: u64 = 0x0040_0000;
    const FILE_OFFSET: usize = 0x1000;
    const PRIVATE_VA: u64 = 0x0010_0000;
    let mut code = [0u8; 69];
    code[0..2].copy_from_slice(&[0x48, 0xB8]); // movabs rax, PRIVATE_VA
    code[2..10].copy_from_slice(&PRIVATE_VA.to_le_bytes());
    code[10..13].copy_from_slice(&[0x48, 0x8B, 0x18]); // mov rbx, [rax]
    code[13..15].copy_from_slice(&[0x48, 0xB9]); // movabs rcx, expected
    code[15..23].copy_from_slice(&expected.to_le_bytes());
    code[23..26].copy_from_slice(&[0x48, 0x39, 0xCB]); // cmp rbx, rcx
    code[26..28].copy_from_slice(&[0x75, 0x1B]); // jne failure
    code[28..33].copy_from_slice(&[0xB8, 2, 0, 0, 0]); // mov eax, TIMER_TICKS
    code[33..35].copy_from_slice(&[0xCD, 0x80]);
    code[35..39].copy_from_slice(&[0x48, 0x83, 0xF8, 30]); // cmp rax, 30
    code[39..41].copy_from_slice(&[0x72, 0xF3]); // jb loop
    code[41..46].copy_from_slice(&[0xBF, 0, 0, 0, 0]); // exit(0)
    code[46..51].copy_from_slice(&[0xB8, 4, 0, 0, 0]);
    code[51..53].copy_from_slice(&[0xCD, 0x80]);
    code[53..55].copy_from_slice(&[0x0F, 0x0B]); // unreachable sentinel
    code[55..60].copy_from_slice(&[0xBF, 1, 0, 0, 0]); // failure: exit(1)
    code[60..65].copy_from_slice(&[0xB8, 4, 0, 0, 0]);
    code[65..67].copy_from_slice(&[0xCD, 0x80]);
    code[67..69].copy_from_slice(&[0x0F, 0x0B]);

    let mut bytes = alloc::vec![0; FILE_OFFSET + code.len()];
    bytes[..16].copy_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    put_u16(&mut bytes, 16, 2);
    put_u16(&mut bytes, 18, 62);
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, ENTRY);
    put_u64(&mut bytes, 32, ELF_HEADER_SIZE as u64);
    put_u16(&mut bytes, 52, ELF_HEADER_SIZE as u16);
    put_u16(&mut bytes, 54, PROGRAM_HEADER_SIZE as u16);
    put_u16(&mut bytes, 56, 1);
    test_load_header(
        &mut bytes,
        64,
        PF_R | PF_X,
        FILE_OFFSET as u64,
        ENTRY,
        code.len() as u64,
        code.len() as u64,
    );
    bytes[FILE_OFFSET..].copy_from_slice(&code);
    bytes
}

fn test_load_header(
    bytes: &mut [u8],
    at: usize,
    flags: u32,
    offset: u64,
    va: u64,
    filesz: u64,
    memsz: u64,
) {
    put_u32(bytes, at, PT_LOAD);
    put_u32(bytes, at + 4, flags);
    put_u64(bytes, at + 8, offset);
    put_u64(bytes, at + 16, va);
    put_u64(bytes, at + 32, filesz);
    put_u64(bytes, at + 40, memsz);
    put_u64(bytes, at + 48, Size4KiB::SIZE);
}

fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
