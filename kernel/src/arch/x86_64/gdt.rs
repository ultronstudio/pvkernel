use spin::Once;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

use crate::kprintln;

/// IST index used by the double-fault handler.
///
/// The double fault handler runs on its own stack (`DOUBLE_FAULT_STACK`)
/// instead of the current one, because a double fault can itself be caused by
/// a stack overflow, which would otherwise fault again while the CPU tries to
/// push the exception frame.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const DOUBLE_FAULT_STACK_SIZE: usize = 5 * 4096;

#[repr(align(16))]
// Backing storage for the double-fault stack: only its address is used, the
// CPU writes to it directly as a stack, so Rust never reads the field.
#[allow(dead_code)]
struct DoubleFaultStack([u8; DOUBLE_FAULT_STACK_SIZE]);

static DOUBLE_FAULT_STACK: DoubleFaultStack = DoubleFaultStack([0; DOUBLE_FAULT_STACK_SIZE]);

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

fn double_fault_stack_top() -> VirtAddr {
    // x86_64 stacks grow downward, so the initial stack pointer is the
    // address immediately past the end of the backing buffer.
    let stack_start = VirtAddr::from_ptr(core::ptr::addr_of!(DOUBLE_FAULT_STACK));
    stack_start + DOUBLE_FAULT_STACK_SIZE as u64
}

/// Installs the pvKernel GDT and TSS and loads them onto the current CPU.
///
/// Must run before [`super::interrupts::init`], because the IDT's
/// double-fault entry references [`DOUBLE_FAULT_IST_INDEX`], which is only
/// meaningful once the TSS below has been loaded into the task register.
pub fn init() {
    kprintln!("[cpu] loading GDT");

    let tss = TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = double_fault_stack_top();
        tss
    });

    let (gdt, selectors) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(tss));
        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
            },
        )
    });

    gdt.load();

    // SAFETY: `code_selector` and `tss_selector` index the kernel code
    // segment and TSS descriptor that were just appended to `gdt` above, and
    // `gdt.load()` has already made this the active GDT, so both selectors
    // are valid for the currently loaded descriptor table. The data/stack
    // segment registers still hold selectors from the bootloader's own GDT;
    // those indexes are meaningless (or now index into the TSS descriptor)
    // in pvKernel's GDT, so they are reset to the null selector, which x86_64
    // long mode accepts unconditionally for SS, DS, ES, FS and GS.
    unsafe {
        CS::set_reg(selectors.code_selector);
        SS::set_reg(SegmentSelector::NULL);
        DS::set_reg(SegmentSelector::NULL);
        ES::set_reg(SegmentSelector::NULL);
        FS::set_reg(SegmentSelector::NULL);
        GS::set_reg(SegmentSelector::NULL);
        load_tss(selectors.tss_selector);
    }

    kprintln!("[cpu] TSS loaded");
}
