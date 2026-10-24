#[cfg(feature = "process-preemption-test")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::{Mutex, Once};
use x86_64::instructions::{hlt, interrupts};

use crate::{
    arch::x86_64::memory::ValidatedUserContext,
    arch::x86_64::{
        gdt,
        pic::{self, IrqLine},
        pit, task as arch_task,
    },
    kprintln,
    process::{self, ProcessId},
};

pub const MAX_KERNEL_TASKS: usize = arch_task::MAX_KERNEL_TASKS;
pub type KernelTaskEntry = fn();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskId {
    slot: u8,
    generation: u64,
}

/// Complete target description for the architecture handoff. Scheduler and
/// process locks are released before this plan is resolved to a root and
/// applied; assembly receives only `context_rsp` after CR3/RSP0 are safe.
#[derive(Clone, Copy)]
struct SwitchPlan {
    context_rsp: u64,
    target: TaskKind,
    task_slot: Option<usize>,
}

impl TaskId {
    pub const fn slot(self) -> usize {
        self.slot as usize
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Vacant,
    Prepared,
    Ready,
    Running,
    Exiting,
    Exited,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskKind {
    Kernel,
    User(ProcessId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    SchedulerNotInitialized,
    TaskTableFull,
    NoTaskStackAvailable,
    QueueFull,
    InvalidUserContext,
    NotPrepared,
    ProcessNotAlive,
    ProcessAddressSpaceMismatch,
    InvalidTask,
}

#[derive(Clone, Copy)]
struct TaskSlot {
    generation: u64,
    state: TaskState,
    kind: TaskKind,
    saved_rsp: u64,
    queued: bool,
}

impl TaskSlot {
    const EMPTY: Self = Self {
        generation: 0,
        state: TaskState::Vacant,
        kind: TaskKind::Kernel,
        saved_rsp: 0,
        queued: false,
    };
}

struct ReadyQueue {
    ids: [Option<TaskId>; MAX_KERNEL_TASKS],
    head: usize,
    len: usize,
}

impl ReadyQueue {
    const fn new() -> Self {
        Self {
            ids: [None; MAX_KERNEL_TASKS],
            head: 0,
            len: 0,
        }
    }
    fn push(&mut self, id: TaskId) -> Result<(), SpawnError> {
        if self.len == MAX_KERNEL_TASKS {
            return Err(SpawnError::QueueFull);
        }
        let tail = (self.head + self.len) % MAX_KERNEL_TASKS;
        assert!(self.ids[tail].is_none(), "runnable queue tail was occupied");
        self.ids[tail] = Some(id);
        self.len += 1;
        Ok(())
    }
    fn pop(&mut self) -> Option<TaskId> {
        if self.len == 0 {
            return None;
        }
        let id = self.ids[self.head].take();
        self.head = (self.head + 1) % MAX_KERNEL_TASKS;
        self.len -= 1;
        id
    }
}

struct Scheduler {
    initialized: bool,
    slots: [TaskSlot; MAX_KERNEL_TASKS],
    queue: ReadyQueue,
    current: Option<TaskId>,
    bootstrap_rsp: u64,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            initialized: false,
            slots: [TaskSlot::EMPTY; MAX_KERNEL_TASKS],
            queue: ReadyQueue::new(),
            current: None,
            bootstrap_rsp: 0,
        }
    }
}

static SCHEDULER: Once<Mutex<Scheduler>> = Once::new();
#[cfg(feature = "process-preemption-test")]
static PROCESS_PREEMPTION_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "process-preemption-test")]
static PROCESS_PREEMPTION_PROGRESS_A: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "process-preemption-test")]
static PROCESS_PREEMPTION_PROGRESS_B: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "process-preemption-test")]
static PROCESS_PREEMPTION_START_TICK: AtomicU64 = AtomicU64::new(0);
static SMOKE_A: AtomicU64 = AtomicU64::new(0);
static SMOKE_B: AtomicU64 = AtomicU64::new(0);
static SMOKE_A_SP: AtomicU64 = AtomicU64::new(0);
static SMOKE_B_SP: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "scheduler-preemption-test")]
static PREEMPT_A_PROGRESS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "scheduler-preemption-test")]
static PREEMPT_B_PROGRESS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "scheduler-preemption-test")]
static PREEMPT_A_DONE: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "scheduler-preemption-test")]
static PREEMPT_B_DONE: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "task-capacity-test")]
static CAPACITY_DONE: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    kprintln!("[scheduler] initializing");
    test_queue_wraparound();
    arch_task::init_stacks();
    arch_task::verify_stack_mappings();
    let scheduler = SCHEDULER.call_once(|| Mutex::new(Scheduler::new()));
    interrupts::without_interrupts(|| {
        let mut scheduler = scheduler.lock();
        assert!(!scheduler.initialized, "scheduler initialized twice");
        scheduler.initialized = true;
    });
    kprintln!("[scheduler] task capacity: {MAX_KERNEL_TASKS}");
    kprintln!("[scheduler] runnable queue initialized");
    kprintln!("[scheduler] initialized");
}

fn test_queue_wraparound() {
    let mut queue = ReadyQueue::new();
    for slot in 0..MAX_KERNEL_TASKS {
        queue
            .push(TaskId {
                slot: slot as u8,
                generation: 1,
            })
            .expect("queue fill failed");
    }
    assert_eq!(
        queue.push(TaskId {
            slot: u8::MAX,
            generation: 1,
        }),
        Err(SpawnError::QueueFull)
    );
    for expected in 0..5 {
        assert_eq!(
            queue.pop(),
            Some(TaskId {
                slot: expected,
                generation: 1
            })
        );
    }
    for slot in MAX_KERNEL_TASKS..MAX_KERNEL_TASKS + 5 {
        queue
            .push(TaskId {
                slot: slot as u8,
                generation: 1,
            })
            .expect("queue wrap push failed");
    }
    for expected in 5..MAX_KERNEL_TASKS + 5 {
        assert_eq!(
            queue.pop(),
            Some(TaskId {
                slot: expected as u8,
                generation: 1
            })
        );
    }
    assert_eq!(queue.pop(), None);
}

pub fn spawn_kernel_task(entry: KernelTaskEntry) -> Result<TaskId, SpawnError> {
    let scheduler = SCHEDULER.get().ok_or(SpawnError::SchedulerNotInitialized)?;
    interrupts::without_interrupts(|| {
        let mut scheduler = scheduler.lock();
        if !scheduler.initialized {
            return Err(SpawnError::SchedulerNotInitialized);
        }
        let mut reusable = None;
        for (index, slot) in scheduler.slots.iter_mut().enumerate() {
            if !matches!(slot.state, TaskState::Vacant | TaskState::Exited) {
                continue;
            }
            if let Some(generation) = slot.generation.checked_add(1) {
                reusable = Some((index, generation));
                break;
            }
            slot.state = TaskState::Retired;
        }
        let (index, generation) = reusable.ok_or(SpawnError::TaskTableFull)?;
        assert!(
            !scheduler.slots[index].queued,
            "reusable task slot still queued"
        );
        let id = TaskId {
            slot: u8::try_from(index).expect("task capacity exceeds TaskId slot"),
            generation,
        };
        let (_, top) = arch_task::stack_bounds(index).ok_or(SpawnError::NoTaskStackAvailable)?;
        let context_size = u64::try_from(core::mem::size_of::<arch_task::Context>())
            .expect("task context size does not fit in u64");
        let context_addr = top
            .checked_sub(context_size)
            .ok_or(SpawnError::NoTaskStackAvailable)?;
        if context_addr % 16 != 0 {
            return Err(SpawnError::NoTaskStackAvailable);
        }
        let context = arch_task::Context {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rdi: entry as usize as u64,
            rsi: 0,
            rbp: 0,
            rdx: 0,
            rcx: 0,
            rbx: 0,
            rax: 0,
            rip: arch_task::task_trampoline(),
            cs: u64::from(gdt::kernel_code_selector().0),
            rflags: 0x202,
            rsp: top,
            ss: 0,
        };
        let (stack_low, _) =
            arch_task::stack_bounds(index).ok_or(SpawnError::NoTaskStackAvailable)?;
        // SAFETY: the slot's mapped pages were verified writable at init;
        // the slot is Vacant/Exited and interrupts are masked under the
        // scheduler lock, so no CPU context can be using this stack. Clear
        // old task data before publishing the slot as Ready.
        let stack_size =
            usize::try_from(arch_task::STACK_SIZE).expect("task stack size does not fit in usize");
        unsafe {
            core::ptr::write_bytes(stack_low as *mut u8, 0, stack_size);
        }
        // SAFETY: init verified this slot's mapped stack pages writable and
        // supervisor-only; the slot is Vacant/Exited and cannot be active.
        unsafe {
            (context_addr as *mut arch_task::Context).write(context);
        }
        scheduler.queue.push(id)?;
        scheduler.slots[index] = TaskSlot {
            generation,
            state: TaskState::Ready,
            kind: TaskKind::Kernel,
            saved_rsp: context_addr,
            queued: true,
        };
        Ok(id)
    })
}

/// Reserves and initializes a user task context without making it runnable.
/// The caller must have already built the process AddressSpace and user stack;
/// `entry` and `user_rsp` are checked scalar values from that validated image.
pub(crate) fn prepare_user_task(
    process_id: ProcessId,
    context: ValidatedUserContext,
) -> Result<TaskId, SpawnError> {
    let root = process::address_space_root(process_id)
        .map_err(|_| SpawnError::ProcessAddressSpaceMismatch)?;
    if context.root_frame() != root {
        return Err(SpawnError::ProcessAddressSpaceMismatch);
    }
    let scheduler = SCHEDULER.get().ok_or(SpawnError::SchedulerNotInitialized)?;
    interrupts::without_interrupts(|| {
        let mut scheduler = scheduler.lock();
        if !scheduler.initialized {
            return Err(SpawnError::SchedulerNotInitialized);
        }
        let index = reserve_slot(&mut scheduler)?;
        let generation = scheduler.slots[index]
            .generation
            .checked_add(1)
            .ok_or(SpawnError::TaskTableFull)?;
        let id = TaskId {
            slot: index as u8,
            generation,
        };
        let (_, top) = arch_task::stack_bounds(index).ok_or(SpawnError::NoTaskStackAvailable)?;
        let context_size = u64::try_from(core::mem::size_of::<arch_task::Context>())
            .map_err(|_| SpawnError::NoTaskStackAvailable)?;
        let context_addr = top
            .checked_sub(context_size)
            .ok_or(SpawnError::NoTaskStackAvailable)?;
        if context_addr % 16 != 0 {
            return Err(SpawnError::NoTaskStackAvailable);
        }
        let context = arch_task::Context {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rdi: 0,
            rsi: 0,
            rbp: 0,
            rdx: 0,
            rcx: 0,
            rbx: 0,
            rax: 0,
            rip: context.entry(),
            cs: u64::from(gdt::user_code_selector().0),
            rflags: 0x202,
            rsp: context.user_rsp(),
            ss: u64::from(gdt::user_data_selector().0),
        };
        let (stack_low, _) =
            arch_task::stack_bounds(index).ok_or(SpawnError::NoTaskStackAvailable)?;
        // SAFETY: scheduler lock and IF masking prove this slot is not active;
        // its pages were verified supervisor RW/NX during task initialization.
        unsafe {
            core::ptr::write_bytes(stack_low as *mut u8, 0, arch_task::STACK_SIZE as usize);
            (context_addr as *mut arch_task::Context).write(context);
        }
        scheduler.slots[index] = TaskSlot {
            generation,
            state: TaskState::Prepared,
            kind: TaskKind::User(process_id),
            saved_rsp: context_addr,
            queued: false,
        };
        Ok(id)
    })
}

/// Atomically publishes the prepared process/task pair. Lock order is
/// scheduler then process table; IF remains clear through Process Alive and
/// ready-queue publication, so neither timer scheduling nor another kernel
/// observer can see an intermediate state on this single CPU.
pub(crate) fn publish_prepared_user_task(id: TaskId) -> Result<(), SpawnError> {
    let scheduler = SCHEDULER.get().ok_or(SpawnError::SchedulerNotInitialized)?;
    interrupts::without_interrupts(|| {
        let mut scheduler = scheduler.lock();
        let slot = scheduler
            .slots
            .get(id.slot())
            .ok_or(SpawnError::InvalidTask)?;
        if slot.generation != id.generation || slot.state != TaskState::Prepared || slot.queued {
            return Err(SpawnError::NotPrepared);
        }
        let TaskKind::User(process_id) = slot.kind else {
            return Err(SpawnError::NotPrepared);
        };
        if scheduler.queue.len == MAX_KERNEL_TASKS {
            return Err(SpawnError::QueueFull);
        }
        process::publish_created(process_id, id).map_err(|_| SpawnError::ProcessNotAlive)?;
        scheduler
            .queue
            .push(id)
            .expect("ready queue changed under scheduler lock");
        let slot = &mut scheduler.slots[id.slot()];
        slot.state = TaskState::Ready;
        slot.queued = true;
        Ok(())
    })
}

#[cfg(feature = "qemu-test")]
pub fn run_prepared_user_task_test() {
    let process_id = process::reserve(None, "prepared-test", pit::ticks())
        .expect("failed to reserve prepared-task test process");
    let (address_space, context) =
        crate::arch::x86_64::memory::with_kernel_address_space(|kernel| {
            let mut address_space = kernel
                .create_address_space()
                .expect("failed to create prepared-task test root");
            crate::arch::x86_64::memory::UserStack::map(&mut address_space, kernel)
                .expect("failed to create prepared-task test stack");
            let page = x86_64::structures::paging::Page::from_start_address(x86_64::VirtAddr::new(
                crate::arch::x86_64::memory::USER_MIN,
            ))
            .expect("USER_MIN is not page aligned");
            address_space
                .map_new_user_page(
                    kernel,
                    page,
                    crate::arch::x86_64::memory::Permissions::USER_CODE,
                )
                .expect("failed to map prepared-task entry page");
            let context = address_space
                .validate_user_context(
                    kernel,
                    crate::arch::x86_64::memory::USER_MIN,
                    crate::arch::x86_64::memory::USER_STACK_TOP,
                )
                .expect("test entry/stack pair did not validate");
            (address_space, context)
        });
    process::install_address_space(process_id, address_space)
        .expect("failed to attach Creating process AddressSpace");
    let id = prepare_user_task(process_id, context).expect("failed to prepare user task");
    process::install_primary_task(process_id, id).expect("failed to attach prepared primary task");
    assert_eq!(task_state(id), Some(TaskState::Prepared));
    assert_eq!(task_kind(id), Some(TaskKind::User(process_id)));
    assert_eq!(task_state(id), Some(TaskState::Prepared));
    assert!(!process::is_alive(process_id));
    let snapshot = process::snapshot(process_id).expect("Creating process snapshot missing");
    assert_eq!(snapshot.state, process::ProcessState::Creating);
    assert_eq!(snapshot.mapped_user_pages, 9);
    cancel_prepared_task(id).expect("could not cancel prepared test task");
    process::abort_creation(process_id).expect("could not roll back test process reservation");
    assert_eq!(task_state(id), None);
    kprintln!("[scheduler-test] prepared user task remains non-runnable until process Alive");
}

pub fn cancel_prepared_task(id: TaskId) -> Result<(), SpawnError> {
    let scheduler = SCHEDULER.get().ok_or(SpawnError::SchedulerNotInitialized)?;
    interrupts::without_interrupts(|| {
        let mut scheduler = scheduler.lock();
        let slot = scheduler
            .slots
            .get_mut(id.slot())
            .ok_or(SpawnError::InvalidTask)?;
        if slot.generation != id.generation || slot.state != TaskState::Prepared || slot.queued {
            return Err(SpawnError::NotPrepared);
        }
        slot.state = TaskState::Vacant;
        slot.saved_rsp = 0;
        slot.kind = TaskKind::Kernel;
        Ok(())
    })
}

pub fn task_kind(id: TaskId) -> Option<TaskKind> {
    let scheduler = SCHEDULER.get()?;
    interrupts::without_interrupts(|| {
        let scheduler = scheduler.lock();
        let slot = scheduler.slots.get(id.slot())?;
        (slot.generation == id.generation && slot.state != TaskState::Vacant).then_some(slot.kind)
    })
}

fn reserve_slot(scheduler: &mut Scheduler) -> Result<usize, SpawnError> {
    for (index, slot) in scheduler.slots.iter_mut().enumerate() {
        if !matches!(slot.state, TaskState::Vacant | TaskState::Exited) {
            continue;
        }
        if slot.generation.checked_add(1).is_some() {
            return Ok(index);
        }
        slot.state = TaskState::Retired;
    }
    Err(SpawnError::TaskTableFull)
}

pub fn task_state(id: TaskId) -> Option<TaskState> {
    let scheduler = SCHEDULER.get()?;
    interrupts::without_interrupts(|| {
        let scheduler = scheduler.lock();
        let slot = scheduler.slots.get(id.slot())?;
        (slot.generation == id.generation && slot.state != TaskState::Vacant).then_some(slot.state)
    })
}

/// Called only by the assembly IRQ0 boundary with IF cleared by the IDT
/// interrupt gate. The pointer is the exact register+hardware frame created
/// by `switch.rs`. Legacy CPL3 tests run from the bootstrap context and retain
/// their old non-scheduling behavior; process-owned user tasks are preempted.
pub(crate) fn timer_schedule(context: *mut arch_task::Context) -> *mut arch_task::Context {
    pit::record_tick();
    // SAFETY: assembly passes the address of its just-saved register prefix;
    // the hardware frame immediately follows it and is readable here.
    let from_user = unsafe { (*context).cs & 3 == 3 };
    #[cfg(feature = "userspace-syscall-test")]
    if from_user {
        crate::arch::x86_64::syscall::test::TIMER_INTERRUPTED_CPL3.store(true, Ordering::Relaxed);
    }
    let scheduler = SCHEDULER
        .get()
        .expect("timer IRQ before scheduler initialization");
    #[cfg(feature = "process-preemption-test")]
    match process_preemption_test_result() {
        Some(true) => {
            kprintln!("[process-test] user A and user B both made progress");
            kprintln!("[process-test] CPL3 timer preemption passed");
            pic::notify_end_of_interrupt(IrqLine::Timer);
            crate::arch::x86_64::qemu::exit_success();
        }
        Some(false) => {
            kprintln!("[process-test] CPL3 preemption tick watchdog expired");
            pic::notify_end_of_interrupt(IrqLine::Timer);
            crate::arch::x86_64::qemu::exit_failure();
        }
        None => {}
    }
    if from_user {
        let process_task_running = interrupts::without_interrupts(|| {
            let scheduler = scheduler.lock();
            scheduler.current.is_some()
        });
        // Historical CPL3 syscall tests enter directly from bootstrap, not
        // through a process Task. Preserve that test context; a real user
        // process always has a current User task.
        if !process_task_running {
            pic::notify_end_of_interrupt(IrqLine::Timer);
            return context;
        }
    }

    let plan = interrupts::without_interrupts(|| {
        let mut scheduler = scheduler.lock();
        assert!(
            scheduler.initialized,
            "timer IRQ before scheduler initialization"
        );
        match scheduler.current {
            Some(id) => {
                let slot = scheduler
                    .slots
                    .get_mut(id.slot())
                    .expect("current task index invalid");
                assert_eq!(
                    slot.generation, id.generation,
                    "current task generation stale"
                );
                assert!(
                    matches!(slot.state, TaskState::Running | TaskState::Exiting),
                    "current task not Running/Exiting"
                );
                assert_eq!(
                    matches!(slot.kind, TaskKind::User(_)),
                    from_user,
                    "saved privilege does not match current task kind"
                );
                assert!(!slot.queued, "Running task appeared in queue");
                slot.saved_rsp = context as u64;
                assert_task_stack_pointer(id.slot(), slot.saved_rsp);
                if slot.state == TaskState::Exiting {
                    slot.state = TaskState::Exited;
                    scheduler.current = None;
                } else {
                    slot.state = TaskState::Ready;
                    slot.queued = true;
                    scheduler.queue.push(id).expect("runnable queue overflow");
                }
            }
            None => scheduler.bootstrap_rsp = context as u64,
        }

        let next = scheduler.queue.pop();
        if let Some(id) = next {
            let slot = scheduler
                .slots
                .get_mut(id.slot())
                .expect("queued task index invalid");
            assert_eq!(
                slot.generation, id.generation,
                "stale TaskId in runnable queue"
            );
            assert_eq!(slot.state, TaskState::Ready, "queued task is not Ready");
            assert!(slot.queued, "queued task missing queued flag");
            slot.queued = false;
            slot.state = TaskState::Running;
            let saved_rsp = slot.saved_rsp;
            assert_task_stack_pointer(id.slot(), saved_rsp);
            let target = slot.kind;
            scheduler.current = Some(id);
            SwitchPlan {
                context_rsp: saved_rsp,
                target,
                task_slot: Some(id.slot()),
            }
        } else {
            scheduler.current = None;
            assert_ne!(
                scheduler.bootstrap_rsp, 0,
                "bootstrap context was never captured"
            );
            SwitchPlan {
                context_rsp: scheduler.bootstrap_rsp,
                target: TaskKind::Kernel,
                task_slot: None,
            }
        }
    });
    pic::notify_end_of_interrupt(IrqLine::Timer);
    apply_switch_plan(plan);
    plan.context_rsp as *mut arch_task::Context
}

/// Resolves the selected process root and performs architecture state changes
/// after scheduler locks are gone. IF is still clear from the interrupt gate;
/// no lock spans EOI, CR3, RSP0, RSP adoption, or iretq.
fn apply_switch_plan(plan: SwitchPlan) {
    let user_target = match plan.target {
        TaskKind::Kernel => None,
        TaskKind::User(process_id) => Some(
            process::address_space_handle(process_id)
                .expect("selected user task has no Alive process address space"),
        ),
    };
    crate::arch::x86_64::memory::with_kernel_address_space(|kernel| {
        match user_target {
            Some(address_space) => kernel
                .activate_handle(address_space)
                .expect("selected process root failed the kernel mapping audit"),
            None => kernel
                .activate_kernel_root()
                .expect("kernel root failed the mapping audit"),
        }
        if let Some(slot) = plan.task_slot.filter(|_| user_target.is_some()) {
            let (_, top) = arch_task::stack_bounds(slot).expect("user task stack slot invalid");
            gdt::set_current_rsp0(top).expect("selected user task has invalid kernel stack");
        } else {
            gdt::set_kernel_fallback_rsp0().expect("kernel fallback RSP0 is unavailable");
        }
    });
}

fn assert_task_stack_pointer(slot: usize, rsp: u64) {
    let (low, high) = arch_task::stack_bounds(slot).expect("invalid task stack slot");
    assert!(
        (low..high).contains(&rsp),
        "task saved RSP outside owned stack"
    );
}

pub(crate) fn task_exit() -> ! {
    let scheduler = SCHEDULER
        .get()
        .expect("task exit before scheduler initialization");
    interrupts::without_interrupts(|| {
        let mut scheduler = scheduler.lock();
        let id = scheduler
            .current
            .expect("bootstrap context cannot exit as a task");
        let slot = scheduler
            .slots
            .get_mut(id.slot())
            .expect("current task index invalid during exit");
        assert_eq!(slot.generation, id.generation);
        assert_eq!(slot.state, TaskState::Running);
        slot.state = TaskState::Exiting;
    });
    // The task remains on its own stack until the next PIT interrupt saves
    // this context and switches elsewhere. STI;HLT is the safe idle pair.
    loop {
        unsafe {
            core::arch::asm!("sti; hlt", options(nomem, nostack));
        }
    }
}

fn smoke_a() {
    let rsp: u64;
    // SAFETY: reading RSP is a non-mutating observation used to verify that
    // the task executes inside its assigned mapped stack.
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
    }
    SMOKE_A_SP.store(rsp, Ordering::Relaxed);
    SMOKE_A.store(1, Ordering::Relaxed);
}
fn smoke_b() {
    let rsp: u64;
    // SAFETY: see `smoke_a`; this only observes the current task stack.
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
    }
    SMOKE_B_SP.store(rsp, Ordering::Relaxed);
    SMOKE_B.store(1, Ordering::Relaxed);
}

pub fn prepare_smoke_test() -> (TaskId, TaskId) {
    SMOKE_A.store(0, Ordering::Relaxed);
    SMOKE_B.store(0, Ordering::Relaxed);
    SMOKE_A_SP.store(0, Ordering::Relaxed);
    SMOKE_B_SP.store(0, Ordering::Relaxed);
    let task_a = spawn_kernel_task(smoke_a).expect("could not create scheduler smoke task A");
    let task_b = spawn_kernel_task(smoke_b).expect("could not create scheduler smoke task B");
    assert_ne!(task_a, task_b, "scheduler issued duplicate TaskIds");
    assert_ne!(
        task_a.slot(),
        task_b.slot(),
        "scheduler reused a live stack slot"
    );
    assert!(task_a.generation() > 0 && task_b.generation() > 0);
    kprintln!("[scheduler-test] task stacks verified");
    (task_a, task_b)
}

pub fn run_smoke_test(task_a: TaskId, task_b: TaskId) {
    let start = pit::ticks();
    while SMOKE_A.load(Ordering::Relaxed) == 0 || SMOKE_B.load(Ordering::Relaxed) == 0 {
        assert!(
            pit::ticks().wrapping_sub(start) < 1000,
            "scheduler smoke timed out"
        );
        hlt();
    }
    for (task, observed_sp) in [
        (task_a, SMOKE_A_SP.load(Ordering::Relaxed)),
        (task_b, SMOKE_B_SP.load(Ordering::Relaxed)),
    ] {
        let (low, high) = arch_task::stack_bounds(task.slot()).expect("invalid task stack slot");
        assert!(
            (low..high).contains(&observed_sp),
            "task stack pointer outside assigned stack"
        );
    }
    let (a_low, a_high) = arch_task::stack_bounds(task_a.slot()).unwrap();
    let (b_low, b_high) = arch_task::stack_bounds(task_b.slot()).unwrap();
    assert!(
        a_high <= b_low || b_high <= a_low,
        "task stack ranges overlap"
    );
    kprintln!("[scheduler-test] task A completed");
    kprintln!("[scheduler-test] task B completed");
    while task_state(task_a) != Some(TaskState::Exited)
        || task_state(task_b) != Some(TaskState::Exited)
    {
        hlt();
    }
    assert_eq!(task_state(task_a), Some(TaskState::Exited));
    assert_eq!(task_state(task_b), Some(TaskState::Exited));
    kprintln!("[scheduler-test] task state transitions passed");
    kprintln!("[scheduler-test] idle resume passed");
    kprintln!("[scheduler-test] completed successfully");
    kprintln!("[scheduler] kernel task smoke passed");
}

#[cfg(feature = "scheduler-preemption-test")]
fn preemption_task_a() {
    let start = pit::ticks();
    while PREEMPT_B_PROGRESS.load(Ordering::Relaxed) == 0 {
        PREEMPT_A_PROGRESS.fetch_add(1, Ordering::Relaxed);
        if pit::ticks().wrapping_sub(start) > 200 {
            kprintln!(
                "[scheduler-test] task A watchdog fired; B progress={}",
                PREEMPT_B_PROGRESS.load(Ordering::Relaxed)
            );
            #[cfg(feature = "qemu-test")]
            crate::arch::x86_64::qemu::exit_failure();
        }
    }
    PREEMPT_A_DONE.store(1, Ordering::Relaxed);
}

#[cfg(feature = "scheduler-preemption-test")]
fn preemption_task_b() {
    let start = pit::ticks();
    loop {
        PREEMPT_B_PROGRESS.fetch_add(1, Ordering::Relaxed);
        if PREEMPT_A_PROGRESS.load(Ordering::Relaxed) >= 100 {
            break;
        }
        if pit::ticks().wrapping_sub(start) > 200 {
            kprintln!(
                "[scheduler-test] task B watchdog fired; A progress={}",
                PREEMPT_A_PROGRESS.load(Ordering::Relaxed)
            );
            #[cfg(feature = "qemu-test")]
            crate::arch::x86_64::qemu::exit_failure();
        }
    }
    PREEMPT_B_DONE.store(1, Ordering::Relaxed);
}

#[cfg(feature = "scheduler-preemption-test")]
pub fn run_preemption_test() {
    PREEMPT_A_PROGRESS.store(0, Ordering::Relaxed);
    PREEMPT_B_PROGRESS.store(0, Ordering::Relaxed);
    PREEMPT_A_DONE.store(0, Ordering::Relaxed);
    PREEMPT_B_DONE.store(0, Ordering::Relaxed);
    let before = pit::ticks();
    let task_a = spawn_kernel_task(preemption_task_a).expect("could not spawn preemption task A");
    let task_b = spawn_kernel_task(preemption_task_b).expect("could not spawn preemption task B");
    assert_ne!(task_a, task_b);
    while PREEMPT_A_DONE.load(Ordering::Relaxed) == 0
        || PREEMPT_B_DONE.load(Ordering::Relaxed) == 0
        || task_state(task_a) != Some(TaskState::Exited)
        || task_state(task_b) != Some(TaskState::Exited)
    {
        hlt();
    }
    assert!(PREEMPT_A_PROGRESS.load(Ordering::Relaxed) > 0);
    assert!(PREEMPT_B_PROGRESS.load(Ordering::Relaxed) > 0);
    assert!(pit::ticks() > before);
    kprintln!("[scheduler-test] timer preemption observed");
    kprintln!("[scheduler-test] task A made progress");
    kprintln!("[scheduler-test] task B made progress");
    kprintln!("[scheduler-test] timer remained monotonic");
    kprintln!("[scheduler-test] preemption completed successfully");
}

#[cfg(feature = "process-preemption-test")]
fn prepare_preemption_process(
    name: &str,
    shared_frame: Option<
        x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>,
    >,
    process_b: bool,
) -> (
    ProcessId,
    u64,
    x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>,
) {
    use x86_64::structures::paging::{Page, PageSize, Size4KiB};

    let process_id = process::reserve(None, name, pit::ticks())
        .expect("failed to reserve process-preemption test process");
    let (address_space, context, progress_alias, progress_frame) =
        crate::arch::x86_64::memory::with_kernel_address_space(|kernel| {
            let mut address_space = kernel
                .create_address_space()
                .expect("failed to create process-preemption test root");
            crate::arch::x86_64::memory::UserStack::map(&mut address_space, kernel)
                .expect("failed to map process-preemption user stack");

            let code_page = Page::<Size4KiB>::from_start_address(x86_64::VirtAddr::new(
                crate::arch::x86_64::memory::USER_MIN,
            ))
            .expect("USER_MIN is not page aligned");
            let progress_address = crate::arch::x86_64::memory::USER_MIN + Size4KiB::SIZE;
            let progress_page =
                Page::<Size4KiB>::from_start_address(x86_64::VirtAddr::new(progress_address))
                    .expect("progress page is not aligned");
            address_space
                .map_new_user_page(
                    kernel,
                    code_page,
                    crate::arch::x86_64::memory::Permissions::USER_DATA,
                )
                .expect("failed to allocate user loop code page");
            let progress_frame = if let Some(frame) = shared_frame {
                address_space
                    .map_test_shared_user_page(kernel, progress_page, frame)
                    .expect("failed to map shared preemption-test progress page");
                frame
            } else {
                address_space
                    .map_new_user_page(
                        kernel,
                        progress_page,
                        crate::arch::x86_64::memory::Permissions::USER_DATA,
                    )
                    .expect("failed to allocate user progress page")
            };

            // Trusted closed loop: increment this process's counter, test the
            // peer's counter, set an observed-peer flag, and repeat. Neither
            // payload invokes a syscall or yields voluntarily.
            let mut code = [0u8; 30];
            code[0..2].copy_from_slice(&[0x48, 0xB8]);
            let own_counter = progress_address + if process_b { 8 } else { 0 };
            code[2..10].copy_from_slice(&own_counter.to_le_bytes());
            code[10..13].copy_from_slice(&[0x48, 0xFF, 0x00]);
            let peer_offset = if process_b { 0xF8 } else { 0x08 };
            code[13..20].copy_from_slice(&[0x48, 0x83, 0x78, peer_offset, 0x01, 0x72, 0xF6]);
            code[20..28].copy_from_slice(&[0x48, 0xC7, 0x40, 0x10, 0x01, 0, 0, 0]);
            code[28..30].copy_from_slice(&[0xEB, 0xEC]);
            address_space
                .write_user_page(kernel, code_page, 0, &code)
                .expect("failed to write process-preemption test payload");
            address_space
                .update_user_permissions(
                    kernel,
                    code_page,
                    crate::arch::x86_64::memory::Permissions::USER_CODE,
                )
                .expect("failed to make test code RX");
            let progress_alias = address_space
                .test_user_page_kernel_alias(kernel, progress_page)
                .expect("failed to obtain progress page supervisor alias")
                .as_u64();
            let context = address_space
                .validate_user_context(
                    kernel,
                    crate::arch::x86_64::memory::USER_MIN,
                    crate::arch::x86_64::memory::USER_STACK_TOP,
                )
                .expect("process-preemption user context failed validation");
            (address_space, context, progress_alias, progress_frame)
        });
    process::install_address_space(process_id, address_space)
        .expect("failed to attach process-preemption address space");
    let task_id = prepare_user_task(process_id, context)
        .expect("failed to prepare process-preemption user task");
    process::install_primary_task(process_id, task_id)
        .expect("failed to attach process-preemption task");
    publish_prepared_user_task(task_id).expect("failed to publish process-preemption task");
    (process_id, progress_alias, progress_frame)
}

/// Adds two non-yielding CPL3 loops to the ready queue. The PIT handler
/// reports QEMU success only after both independent user pages advanced.
#[cfg(feature = "process-preemption-test")]
pub fn prepare_process_preemption_test() {
    PROCESS_PREEMPTION_ACTIVE.store(false, Ordering::Relaxed);
    let (_, alias_a, progress_frame) = prepare_preemption_process("preempt-a", None, false);
    let (_, alias_b, _) = prepare_preemption_process("preempt-b", Some(progress_frame), true);
    PROCESS_PREEMPTION_PROGRESS_A.store(alias_a, Ordering::Relaxed);
    PROCESS_PREEMPTION_PROGRESS_B.store(alias_b, Ordering::Relaxed);
    PROCESS_PREEMPTION_START_TICK.store(pit::ticks(), Ordering::Relaxed);
    PROCESS_PREEMPTION_ACTIVE.store(true, Ordering::Release);
    kprintln!("[process-test] two non-yielding user loops queued");
}

#[cfg(feature = "process-preemption-test")]
pub fn wait_for_process_preemption_test() {
    while PROCESS_PREEMPTION_ACTIVE.load(Ordering::Acquire) {
        hlt();
    }
}

#[cfg(feature = "process-preemption-test")]
fn process_preemption_test_result() -> Option<bool> {
    if !PROCESS_PREEMPTION_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let alias_a = PROCESS_PREEMPTION_PROGRESS_A.load(Ordering::Relaxed);
    let alias_b = PROCESS_PREEMPTION_PROGRESS_B.load(Ordering::Relaxed);
    if alias_a == 0 || alias_b == 0 {
        return None;
    }
    // Both processes map this zeroed test page at the same user VA. Their
    // closed loops set distinct flags only after reading the peer's counter.
    // SAFETY: aliases refer to the allocator-owned page captured during
    // Creating; IF is clear and only the trusted test loops write these
    // aligned qwords. Each offset remains inside the page.
    let process_a_saw_b = unsafe { ((alias_a + 16) as *const u64).read_volatile() };
    // SAFETY: alias_b denotes the same mapped page and +24 is still in-page.
    let process_b_saw_a = unsafe { ((alias_b + 24) as *const u64).read_volatile() };
    if process_a_saw_b != 0 && process_b_saw_a != 0 {
        return Some(true);
    }
    (pit::ticks().saturating_sub(PROCESS_PREEMPTION_START_TICK.load(Ordering::Relaxed)) >= 100)
        .then_some(false)
}

#[cfg(feature = "task-capacity-test")]
fn capacity_task() {
    CAPACITY_DONE.fetch_add(1, Ordering::Relaxed);
}

#[cfg(feature = "task-capacity-test")]
pub fn run_capacity_test() {
    CAPACITY_DONE.store(0, Ordering::Relaxed);
    let mut ids = [None; MAX_KERNEL_TASKS];
    for id in &mut ids {
        *id = Some(spawn_kernel_task(capacity_task).expect("task capacity filled too early"));
    }
    assert_eq!(
        spawn_kernel_task(capacity_task),
        Err(SpawnError::TaskTableFull)
    );
    kprintln!("[scheduler-test] capacity limit passed");
    while CAPACITY_DONE.load(Ordering::Relaxed) < MAX_KERNEL_TASKS as u64
        || ids
            .iter()
            .flatten()
            .any(|id| task_state(*id) != Some(TaskState::Exited))
    {
        hlt();
    }
    let stale = ids[0].expect("capacity task id missing");
    let replacement = spawn_kernel_task(capacity_task).expect("exited task slot was not reusable");
    assert_eq!(replacement.slot(), stale.slot());
    assert!(replacement.generation() > stale.generation());
    assert_eq!(task_state(stale), None);
    kprintln!("[scheduler-test] task slot reuse passed");
    kprintln!("[scheduler-test] stale TaskId rejected");
    while CAPACITY_DONE.load(Ordering::Relaxed) < MAX_KERNEL_TASKS as u64 + 1
        || task_state(replacement) != Some(TaskState::Exited)
    {
        hlt();
    }
}

#[cfg(feature = "task-stack-guard-fault-test")]
pub fn run_stack_guard_fault_test() -> ! {
    fn fault_task() {
        let id = {
            let scheduler = SCHEDULER.get().expect("scheduler not initialized");
            interrupts::without_interrupts(|| scheduler.lock().current.expect("no current task"))
        };
        let (low, _) = arch_task::stack_bounds(id.slot()).expect("invalid task stack slot");
        let guard = low
            .checked_sub(4096)
            .expect("task stack lower guard underflow");
        crate::arch::x86_64::interrupts::arm_expected_task_stack_guard_fault(guard);
        kprintln!("[scheduler-test] triggering expected task stack guard-page fault");
        // SAFETY: this deliberate read targets the task's verified unmapped
        // lower guard and is only called in its dedicated QEMU fault feature.
        unsafe {
            core::ptr::read_volatile(guard as *const u8);
        }
    }
    spawn_kernel_task(fault_task).expect("could not spawn stack guard fault task");
    loop {
        hlt();
    }
}
