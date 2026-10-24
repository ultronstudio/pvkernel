/// General registers followed by the hardware long-mode interrupt frame.
/// The timer entry's push order and these offsets are kept in sync here.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

const _: () = assert!(core::mem::size_of::<Context>() == 20 * 8);
const _: () = assert!(core::mem::offset_of!(Context, rip) == 15 * 8);
const _: () = assert!(core::mem::offset_of!(Context, cs) == 16 * 8);
const _: () = assert!(core::mem::offset_of!(Context, rflags) == 17 * 8);
const _: () = assert!(core::mem::offset_of!(Context, rsp) == 18 * 8);
const _: () = assert!(core::mem::offset_of!(Context, ss) == 19 * 8);
