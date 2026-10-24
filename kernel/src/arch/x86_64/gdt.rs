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

/// Size of the dedicated ring-transition kernel stack installed at
/// `TSS.privilege_stack_table[0]` (RSP0): the stack the CPU switches to when
/// a CPL3-to-CPL0 interrupt or exception occurs (in particular, the `int
/// 0x80` syscall gate; see `arch::x86_64::syscall`).
///
/// This static stack is the bootstrap/kernel-task fallback. User tasks select
/// their scheduler-owned kernel stack through the checked RSP0 setter below.
/// It is sized generously (16 KiB) for the hardware interrupt frame,
/// the syscall entry stub's saved general-purpose registers, the Rust
/// dispatcher's own stack usage, and a nested hardware interrupt (e.g. the
/// PIT timer) firing while the syscall path is on this stack; there is no
/// guard page (see the module-level safety notes below and
/// [docs/architecture/userspace-entry.md](../../../../../docs/architecture/userspace-entry.md)).
const SYSCALL_STACK_SIZE: usize = 4 * 4096;

/// Backing storage for the ring-transition (RSP0) kernel stack.
///
/// `UnsafeCell`-wrapped for the same reason as [`DoubleFaultStack`]: the CPU
/// writes the interrupt frame directly into this memory on every CPL3-to-CPL0
/// transition, and the `int 0x80` entry stub (`arch::x86_64::syscall::entry`)
/// writes its saved register frame here too. This is not itself a
/// synchronization primitive — pvKernel is single-core in 0.0.6, and nothing
/// but the CPU's own privilege-transition mechanism and the syscall entry
/// stub ever touch this memory — `UnsafeCell` only documents that external,
/// non-Rust mutation contract, matching the pattern already used for
/// [`DoubleFaultStack`].
#[repr(align(16))]
struct SyscallStack(UnsafeCell<[u8; SYSCALL_STACK_SIZE]>);

// SAFETY: see `DoubleFaultStack`'s `Sync` impl above; the same single-CPU,
// single-owner reasoning applies here.
unsafe impl Sync for SyscallStack {}

static SYSCALL_STACK: SyscallStack = SyscallStack(UnsafeCell::new([0; SYSCALL_STACK_SIZE]));

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
}

struct TssCell(UnsafeCell<TaskStateSegment>);

// SAFETY: pvKernel is single-core. The CPU reads RSP0 only on a privilege
// transition from CPL3; updates are restricted to CPL0 with IF clear, when no
// such transition can concurrently use the TSS. Other TSS fields are
// initialized once and never mutated after `ltr`.
unsafe impl Sync for TssCell {}

static TSS: Once<TssCell> = Once::new();
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

/// Top-of-stack address for the RSP0 ring-transition stack. `#[repr(align(16))]`
/// on [`SyscallStack`] guarantees the start address is 16-byte aligned, and
/// `SYSCALL_STACK_SIZE` is a multiple of 16, so the returned address is also
/// 16-byte aligned. The `int 0x80` entry stub's stack-alignment reasoning
/// (see `arch::x86_64::syscall::entry`) depends on this.
fn syscall_stack_top() -> VirtAddr {
    // SAFETY: same reasoning as `double_fault_stack_top`: this reads only the
    // storage's address, never a Rust reference.
    let stack_start = VirtAddr::from_ptr(SYSCALL_STACK.0.get());
    let top = stack_start + SYSCALL_STACK_SIZE as u64;
    debug_assert_eq!(
        top.as_u64() % 16,
        0,
        "RSP0 stack top is not 16-byte aligned"
    );
    top
}

/// Confirms the RSP0 stack's backing storage is genuinely writable before it
/// is ever installed in the TSS. See [`verify_double_fault_stack_writable`]
/// for why this check exists and what it does (and does not) prove.
fn verify_syscall_stack_writable() {
    // SAFETY: touches only the last byte of a stack region not yet installed
    // in the TSS and therefore not yet reachable by the CPU or the syscall
    // entry stub; no other code runs concurrently at this point in boot. The
    // original byte is restored immediately.
    unsafe {
        let probe = (SYSCALL_STACK.0.get() as *mut u8).add(SYSCALL_STACK_SIZE - 1);
        let original = probe.read_volatile();
        probe.write_volatile(!original);
        assert_eq!(
            probe.read_volatile(),
            !original,
            "RSP0 stack storage is not writable"
        );
        probe.write_volatile(original);
    }
}

/// Returns the inclusive-exclusive `[bottom, top)` address range of the RSP0
/// ring-transition stack, for test-only verification that the syscall entry
/// path actually executed on this stack rather than an arbitrary one.
#[cfg(feature = "userspace-test")]
pub fn rsp0_stack_range() -> (u64, u64) {
    let top = syscall_stack_top().as_u64();
    (top - SYSCALL_STACK_SIZE as u64, top)
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
    verify_syscall_stack_writable();

    let tss = TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = double_fault_stack_top();
        tss.privilege_stack_table[0] = syscall_stack_top();
        TssCell(UnsafeCell::new(tss))
    });

    let (gdt, selectors) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        // SAFETY: this is the one initialized TSS cell, and its address is
        // stable for kernel lifetime; the GDT descriptor stores that address.
        let tss_ref = unsafe { &*tss.0.get() };
        let tss_selector = gdt.append(Descriptor::tss_segment(tss_ref));
        // `Descriptor::user_code_segment()` / `user_data_segment()` already
        // encode DPL 3; `GlobalDescriptorTable::append` derives the
        // returned `SegmentSelector`'s RPL from that same descriptor DPL,
        // so both selectors below are RPL 3 without any magic-number
        // reconstruction. See docs/architecture/userspace-entry.md for the
        // DPL/RPL/CPL/RPL-requested distinction.
        let user_code_selector = gdt.append(Descriptor::user_code_segment());
        let user_data_selector = gdt.append(Descriptor::user_data_segment());
        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
                user_code_selector,
                user_data_selector,
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
    kprintln!("[tss] ring-transition stack initialized (RSP0)");
    kprintln!(
        "[gdt] user code selector initialized: {:#06x}",
        selectors.user_code_selector.0
    );
    kprintln!(
        "[gdt] user data selector initialized: {:#06x}",
        selectors.user_data_selector.0
    );
}

/// Installs the kernel-entry stack belonging to one allocated scheduler
/// stack slot as the next CPL3-to-CPL0 RSP0 destination.
///
/// The caller must keep interrupts disabled through the later context
/// adoption and `iretq`. Only exact tops returned by the task stack layout
/// are accepted; those stacks were mapped supervisor RW/NX with guards by
/// `task::init_stacks`.
pub fn set_current_rsp0(stack_top: u64) -> Result<(), Rsp0Error> {
    if x86_64::instructions::interrupts::are_enabled() {
        return Err(Rsp0Error::InterruptsEnabled);
    }
    let top = VirtAddr::try_new(stack_top).map_err(|_| Rsp0Error::InvalidAddress)?;
    if stack_top % 16 != 0 {
        return Err(Rsp0Error::InvalidAddress);
    }
    if !crate::arch::x86_64::task::is_validated_stack_top(stack_top) {
        return Err(Rsp0Error::NotTaskStackTop);
    }
    write_rsp0(top)
}

/// Restores the static, kernel-lifetime privilege-transition stack for
/// bootstrap and kernel-task execution. It must be selected before a user
/// task's kernel stack becomes inactive or reusable.
pub fn set_kernel_fallback_rsp0() -> Result<(), Rsp0Error> {
    if x86_64::instructions::interrupts::are_enabled() {
        return Err(Rsp0Error::InterruptsEnabled);
    }
    write_rsp0(kernel_fallback_rsp0())
}

pub fn kernel_fallback_rsp0() -> VirtAddr {
    syscall_stack_top()
}

/// Reads the programmed privilege-transition stack top. Intended for
/// diagnostics and deterministic architecture tests; scheduler callers still
/// select only validated tops through the setters above.
pub fn current_rsp0() -> Result<u64, Rsp0Error> {
    let tss = TSS.get().ok_or(Rsp0Error::TssUnavailable)?;
    // SAFETY: RSP0 is a scalar read. Single-core execution and the existing
    // IF-masked setter prevent a concurrent Rust update; hardware reads it
    // only during a CPL3 privilege transition.
    Ok(unsafe { (*tss.0.get()).privilege_stack_table[0].as_u64() })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rsp0Error {
    InterruptsEnabled,
    InvalidAddress,
    NotTaskStackTop,
    TssUnavailable,
}

fn write_rsp0(top: VirtAddr) -> Result<(), Rsp0Error> {
    let tss = TSS.get().ok_or(Rsp0Error::TssUnavailable)?;
    // SAFETY: called only on the single CPU with IF clear. The CPU is in CPL0
    // and cannot be performing a CPL3 RSP0 transition concurrently; only the
    // RSP0 field changes, while the TSS/GDT storage remains kernel-lifetime.
    unsafe { (*tss.0.get()).privilege_stack_table[0] = top };
    Ok(())
}

/// Returns the ring-3 user code segment selector (RPL 3) installed by
/// [`init`]. Used only when constructing an `iretq` frame for entering
/// CPL3; see `arch::x86_64::userspace`.
///
/// # Panics
///
/// Panics if called before [`init`] has run.
#[cfg_attr(not(feature = "userspace-test"), allow(dead_code))]
pub fn user_code_selector() -> SegmentSelector {
    GDT.get()
        .expect("gdt::init has not run yet")
        .1
        .user_code_selector
}

/// Returns the ring-0 code selector installed by [`init`], for constructing
/// synthetic kernel-task interrupt-return frames.
pub fn kernel_code_selector() -> SegmentSelector {
    GDT.get()
        .expect("gdt::init has not run yet")
        .1
        .code_selector
}

/// Returns the ring-3 user data segment selector (RPL 3) installed by
/// [`init`]. Used for the `SS` (and, informally, `DS`/`ES`) value in an
/// `iretq` frame that enters CPL3.
///
/// # Panics
///
/// Panics if called before [`init`] has run.
#[cfg_attr(not(feature = "userspace-test"), allow(dead_code))]
pub fn user_data_selector() -> SegmentSelector {
    GDT.get()
        .expect("gdt::init has not run yet")
        .1
        .user_data_selector
}
