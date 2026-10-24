//! Bounded, generation-safe process identity and lifecycle state.
//!
//! Architecture-specific address-space and task resources are attached in
//! later layers; this module owns only process policy and bounded metadata.

use spin::{Mutex, Once};

use crate::kprintln;

pub const MAX_PROCESSES: usize = 8;
pub const PROCESS_NAME_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessId {
    slot: u8,
    generation: u64,
}

impl ProcessId {
    pub const fn slot(self) -> usize {
        self.slot as usize
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Pid(u64);

impl Pid {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessName {
    bytes: [u8; PROCESS_NAME_CAPACITY],
    len: u8,
}

impl ProcessName {
    pub const EMPTY: Self = Self {
        bytes: [0; PROCESS_NAME_CAPACITY],
        len: 0,
    };

    pub fn new(value: &str) -> Result<Self, ProcessError> {
        if value.is_empty() || value.len() > PROCESS_NAME_CAPACITY {
            return Err(ProcessError::InvalidName);
        }
        let mut bytes = [0; PROCESS_NAME_CAPACITY];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: u8::try_from(value.len()).map_err(|_| ProcessError::InvalidName)?,
        })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize])
            .expect("ProcessName bytes were validated at construction")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Vacant,
    Creating,
    Alive,
    Exiting,
    Zombie,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessExitReason {
    Exited {
        status: i32,
    },
    Fault {
        vector: u8,
        error_code: Option<u64>,
        fault_address: Option<u64>,
        rip: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessError {
    TableFull,
    PidExhausted,
    InvalidId,
    InvalidTransition,
    InvalidName,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessSnapshot {
    pub id: ProcessId,
    pub pid: Pid,
    pub parent: Option<ProcessId>,
    pub state: ProcessState,
    pub name: ProcessName,
    pub created_tick: u64,
    pub exit_tick: Option<u64>,
    pub cpu_ticks: u64,
    pub mapped_user_pages: u64,
    pub task_count: u8,
    pub exit_reason: Option<ProcessExitReason>,
}

#[derive(Clone, Copy)]
struct ProcessSlot {
    generation: u64,
    state: ProcessState,
    pid: Option<Pid>,
    parent: Option<ProcessId>,
    name: ProcessName,
    created_tick: u64,
    exit_tick: Option<u64>,
    cpu_ticks: u64,
    mapped_user_pages: u64,
    task_count: u8,
    exit_reason: Option<ProcessExitReason>,
}

impl ProcessSlot {
    const EMPTY: Self = Self {
        generation: 0,
        state: ProcessState::Vacant,
        pid: None,
        parent: None,
        name: ProcessName::EMPTY,
        created_tick: 0,
        exit_tick: None,
        cpu_ticks: 0,
        mapped_user_pages: 0,
        task_count: 0,
        exit_reason: None,
    };

    fn clear_to_vacant(&mut self) {
        self.state = ProcessState::Vacant;
        self.pid = None;
        self.parent = None;
        self.name = ProcessName::EMPTY;
        self.created_tick = 0;
        self.exit_tick = None;
        self.cpu_ticks = 0;
        self.mapped_user_pages = 0;
        self.task_count = 0;
        self.exit_reason = None;
    }

    fn snapshot(&self, id: ProcessId) -> ProcessSnapshot {
        ProcessSnapshot {
            id,
            pid: self.pid.expect("live process slot has no PID"),
            parent: self.parent,
            state: self.state,
            name: self.name,
            created_tick: self.created_tick,
            exit_tick: self.exit_tick,
            cpu_ticks: self.cpu_ticks,
            mapped_user_pages: self.mapped_user_pages,
            task_count: self.task_count,
            exit_reason: self.exit_reason,
        }
    }
}

struct ProcessTable {
    slots: [ProcessSlot; MAX_PROCESSES],
    next_pid: Option<u64>,
}

impl ProcessTable {
    const fn new() -> Self {
        Self {
            slots: [ProcessSlot::EMPTY; MAX_PROCESSES],
            next_pid: Some(1),
        }
    }

    fn reserve(
        &mut self,
        parent: Option<ProcessId>,
        name: ProcessName,
        created_tick: u64,
    ) -> Result<ProcessId, ProcessError> {
        let mut available = None;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.state != ProcessState::Vacant {
                continue;
            }
            let Some(generation) = slot.generation.checked_add(1) else {
                slot.state = ProcessState::Retired;
                continue;
            };
            available = Some((index, generation));
            break;
        }
        let (index, generation) = available.ok_or(ProcessError::TableFull)?;

        let Some(raw_pid) = self.next_pid else {
            // The generation is consumed for every reservation attempt, even
            // when boot-lifetime PID space has already been exhausted.
            self.slots[index].generation = generation;
            return Err(ProcessError::PidExhausted);
        };
        self.next_pid = raw_pid.checked_add(1);
        let pid = Pid(raw_pid);
        let id = ProcessId {
            slot: u8::try_from(index).expect("process capacity exceeds ProcessId slot"),
            generation,
        };
        self.slots[index] = ProcessSlot {
            generation,
            state: ProcessState::Creating,
            pid: Some(pid),
            parent,
            name,
            created_tick,
            ..ProcessSlot::EMPTY
        };
        Ok(id)
    }

    fn slot_for(&self, id: ProcessId) -> Result<&ProcessSlot, ProcessError> {
        let slot = self.slots.get(id.slot()).ok_or(ProcessError::InvalidId)?;
        if slot.generation != id.generation
            || matches!(slot.state, ProcessState::Vacant | ProcessState::Retired)
        {
            return Err(ProcessError::InvalidId);
        }
        Ok(slot)
    }

    fn slot_for_mut(&mut self, id: ProcessId) -> Result<&mut ProcessSlot, ProcessError> {
        let slot = self
            .slots
            .get_mut(id.slot())
            .ok_or(ProcessError::InvalidId)?;
        if slot.generation != id.generation
            || matches!(slot.state, ProcessState::Vacant | ProcessState::Retired)
        {
            return Err(ProcessError::InvalidId);
        }
        Ok(slot)
    }

    fn transition(
        &mut self,
        id: ProcessId,
        expected: ProcessState,
        next: ProcessState,
    ) -> Result<(), ProcessError> {
        if !matches!(
            (expected, next),
            (ProcessState::Creating, ProcessState::Alive)
                | (ProcessState::Exiting, ProcessState::Zombie)
        ) {
            return Err(ProcessError::InvalidTransition);
        }
        let slot = self.slot_for_mut(id)?;
        if slot.state != expected {
            return Err(ProcessError::InvalidTransition);
        }
        slot.state = next;
        Ok(())
    }

    fn begin_exit(
        &mut self,
        id: ProcessId,
        reason: ProcessExitReason,
        tick: u64,
    ) -> Result<(), ProcessError> {
        let slot = self.slot_for_mut(id)?;
        if slot.state != ProcessState::Alive {
            return Err(ProcessError::InvalidTransition);
        }
        slot.state = ProcessState::Exiting;
        slot.exit_reason = Some(reason);
        slot.exit_tick = Some(tick);
        Ok(())
    }

    fn reap(&mut self, id: ProcessId) -> Result<(), ProcessError> {
        let slot = self.slot_for_mut(id)?;
        if slot.state != ProcessState::Zombie {
            return Err(ProcessError::InvalidTransition);
        }
        // Generation changes only when a later reservation reuses this
        // Vacant slot. Vacant itself invalidates this ProcessId immediately.
        slot.clear_to_vacant();
        Ok(())
    }

    fn snapshot(&self, id: ProcessId) -> Result<ProcessSnapshot, ProcessError> {
        let slot = self.slot_for(id)?;
        Ok(slot.snapshot(id))
    }

    fn add_cpu_tick(&mut self, id: ProcessId) -> Result<(), ProcessError> {
        let slot = self.slot_for_mut(id)?;
        if slot.state != ProcessState::Alive {
            return Err(ProcessError::InvalidTransition);
        }
        slot.cpu_ticks = slot.cpu_ticks.saturating_add(1);
        Ok(())
    }
}

static PROCESS_TABLE: Once<Mutex<ProcessTable>> = Once::new();

pub fn init() {
    kprintln!("[process] initializing");
    PROCESS_TABLE.call_once(|| Mutex::new(ProcessTable::new()));
    run_model_self_tests();
    kprintln!("[process] process capacity: {MAX_PROCESSES}");
    kprintln!("[process] identity and lifecycle initialized");
}

fn with_table<R>(f: impl FnOnce(&mut ProcessTable) -> R) -> R {
    let table = PROCESS_TABLE
        .get()
        .expect("process subsystem used before init");
    // Callers execute on the single CPU; mask IRQs so a timer cannot
    // re-enter while process metadata is mutably borrowed.
    x86_64::instructions::interrupts::without_interrupts(|| f(&mut table.lock()))
}

pub fn reserve(
    parent: Option<ProcessId>,
    name: &str,
    created_tick: u64,
) -> Result<ProcessId, ProcessError> {
    let name = ProcessName::new(name)?;
    with_table(|table| table.reserve(parent, name, created_tick))
}

pub fn publish_alive(id: ProcessId) -> Result<(), ProcessError> {
    with_table(|table| table.transition(id, ProcessState::Creating, ProcessState::Alive))
}

pub fn begin_exit(id: ProcessId, reason: ProcessExitReason, tick: u64) -> Result<(), ProcessError> {
    with_table(|table| table.begin_exit(id, reason, tick))
}

pub fn mark_zombie(id: ProcessId) -> Result<(), ProcessError> {
    with_table(|table| table.transition(id, ProcessState::Exiting, ProcessState::Zombie))
}

pub fn reap(id: ProcessId) -> Result<(), ProcessError> {
    with_table(|table| table.reap(id))
}

pub fn snapshot(id: ProcessId) -> Result<ProcessSnapshot, ProcessError> {
    let table = PROCESS_TABLE
        .get()
        .expect("process subsystem used before init");
    x86_64::instructions::interrupts::without_interrupts(|| table.lock().snapshot(id))
}

fn run_model_self_tests() {
    let mut table = ProcessTable::new();
    let name = ProcessName::new("model-test").unwrap();
    let first = table.reserve(None, name, 10).unwrap();
    assert_eq!(first.generation(), 1);
    assert_eq!(table.snapshot(first).unwrap().pid.get(), 1);
    assert_eq!(
        table.transition(first, ProcessState::Creating, ProcessState::Zombie),
        Err(ProcessError::InvalidTransition)
    );
    table
        .transition(first, ProcessState::Creating, ProcessState::Alive)
        .unwrap();
    table
        .begin_exit(first, ProcessExitReason::Exited { status: 7 }, 12)
        .unwrap();
    assert_eq!(table.reap(first), Err(ProcessError::InvalidTransition));
    table
        .transition(first, ProcessState::Exiting, ProcessState::Zombie)
        .unwrap();
    table.reap(first).unwrap();
    assert_eq!(table.snapshot(first), Err(ProcessError::InvalidId));
    let second = table.reserve(None, name, 20).unwrap();
    assert_eq!(second.slot(), first.slot());
    assert_eq!(second.generation(), first.generation() + 1);
    assert_eq!(table.snapshot(second).unwrap().pid.get(), 2);
    assert_eq!(table.snapshot(first), Err(ProcessError::InvalidId));
    assert!(table.add_cpu_tick(second).is_err());
    assert_eq!(
        table.snapshot(ProcessId {
            slot: u8::MAX,
            generation: 1
        }),
        Err(ProcessError::InvalidId)
    );

    table
        .transition(second, ProcessState::Creating, ProcessState::Alive)
        .unwrap();
    table
        .begin_exit(
            second,
            ProcessExitReason::Fault {
                vector: 6,
                error_code: None,
                fault_address: None,
                rip: 0x400000,
            },
            22,
        )
        .unwrap();
    assert!(matches!(
        table.snapshot(second).unwrap().exit_reason,
        Some(ProcessExitReason::Fault { vector: 6, .. })
    ));

    table.slots[1].generation = u64::MAX;
    let next = table.reserve(None, name, 21).unwrap();
    assert_ne!(next.slot(), 1);
    assert_eq!(table.slots[1].state, ProcessState::Retired);

    let mut full = ProcessTable::new();
    for _ in 0..MAX_PROCESSES {
        full.reserve(None, name, 0).unwrap();
    }
    assert_eq!(full.reserve(None, name, 0), Err(ProcessError::TableFull));

    let mut pid_exhausted = ProcessTable::new();
    pid_exhausted.next_pid = None;
    assert_eq!(
        pid_exhausted.reserve(None, name, 0),
        Err(ProcessError::PidExhausted)
    );
    assert_eq!(pid_exhausted.slots[0].generation, 1);
    assert_eq!(pid_exhausted.slots[0].state, ProcessState::Vacant);

    let mut last_pid = ProcessTable::new();
    last_pid.next_pid = Some(u64::MAX);
    let final_id = last_pid.reserve(None, name, 0).unwrap();
    assert_eq!(last_pid.snapshot(final_id).unwrap().pid.get(), u64::MAX);
    assert_eq!(
        last_pid.reserve(None, name, 1),
        Err(ProcessError::PidExhausted)
    );
}
