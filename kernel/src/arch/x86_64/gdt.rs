use core::cell::UnsafeCell;

use spin::Once;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

use crate::kprintln;

/// Rust-side (zero-based) index into [`TaskStateSegment::interrupt_stack_table`]
/// used by the double-fault handler.
///
/// This is an array index, not the hardware IST slot number: the x86_64 IDT
/// gate encodes "use IST stack" as a value 1-7, with 0 meaning "no IST
/// stack". The `x86_64` crate's [`x86_64::structures::idt::EntryOptions::set_stack_index`]
/// takes the zero-based Rust index used here and performs the `+1` hardware
/// encoding itself, so `DOUBLE_FAULT_IST_INDEX = 0` correctly selects the
/// first TSS IST entry (hardware slot 1). It must not be changed to `1` to
/// "match" the hardware encoding, since that would instead select the TSS's
/// second IST entry, which is never initialized.
///
/// The double fault handler runs on its own stack (`DOUBLE_FAULT_STACK`)
/// instead of the current one, because a double fault can itself be caused by
/// a stack overflow, which would otherwise fault again while the CPU tries to
/// push the exception frame.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const DOUBLE_FAULT_STACK_SIZE: usize = 5 * 4096;

/// Backing storage for the double-fault stack.
///
/// Wrapped in `UnsafeCell` because the CPU writes the exception frame (and
/// the handler's own execution) directly into this memory once it is
/// installed in the TSS; a plain immutable `static` has no such external
/// mutation contract and the compiler would be free to place it in
/// read-only storage or assume its contents never change.
#[repr(align(16))]
struct DoubleFaultStack(UnsafeCell<[u8; DOUBLE_FAULT_STACK_SIZE]>);

// SAFETY: pvKernel currently initializes and runs on exactly one CPU, so
// there is no concurrent access from multiple execution contexts. `gdt::init`
// only ever reads the storage's address (never a Rust reference) to install
// it into the TSS; after that, Rust code never accesses this cell again, so
// there is no aliasing between Rust and the CPU's own writes when a double
// fault occurs.
unsafe impl Sync for DoubleFaultStack {}

static DOUBLE_FAULT_STACK: DoubleFaultStack =
    DoubleFaultStack(UnsafeCell::new([0; DOUBLE_FAULT_STACK_SIZE]));

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

fn double_fault_stack_top() -> VirtAddr {
    // SAFETY: `UnsafeCell::get` returns a raw pointer without creating a
    // Rust reference to the storage, so this does not conflict with the
    // CPU's later writes when a double fault occurs.
    let stack_start = VirtAddr::from_ptr(DOUBLE_FAULT_STACK.0.get());
    // x86_64 stacks grow downward, so the initial stack pointer is the
    // address immediately past the end of the backing buffer.
    stack_start + DOUBLE_FAULT_STACK_SIZE as u64
}

/// Confirms the double-fault stack's backing storage is genuinely writable
/// before it is ever installed in the TSS or used by the CPU.
///
/// This does not prove correctness under a real double fault (that would
/// require deliberately corrupting kernel state), but it does catch the
/// specific defect this function guards against: the storage silently ending
/// up in read-only memory, which a real double fault would only reveal as an
/// unrecoverable triple fault.
fn verify_double_fault_stack_writable() {
    // SAFETY: this touches only the last byte of a stack region that is not
    // yet installed in the TSS and therefore not yet reachable by the CPU;
    // no other Rust code runs concurrently at this point in initialization.
    // The original byte is restored immediately, so the probe leaves no
    // observable change to the stack contents used later.
    unsafe {
        let probe = (DOUBLE_FAULT_STACK.0.get() as *mut u8).add(DOUBLE_FAULT_STACK_SIZE - 1);
        let original = probe.read_volatile();
        probe.write_volatile(!original);
        assert_eq!(
            probe.read_volatile(),
            !original,
            "double-fault stack storage is not writable"
        );
        probe.write_volatile(original);
    }
}

/// Installs the pvKernel GDT and TSS and loads them onto the current CPU.
///
/// Must run before [`super::interrupts::init`], because the IDT's
/// double-fault entry references [`DOUBLE_FAULT_IST_INDEX`], which is only
/// meaningful once the TSS below has been loaded into the task register.
pub fn init() {
    kprintln!("[cpu] loading GDT");

    verify_double_fault_stack_writable();

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
