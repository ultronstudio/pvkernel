//! Fixed-capacity per-process descriptors for the read-only VFS.

use spin::Mutex;

use crate::{
    fs::vfs::{FileKind, FsError, Node},
    process::{MAX_PROCESSES, ProcessId},
};

pub const MAX_FILES_PER_PROCESS: usize = 8;
const MAX_OPEN_FILES: usize = MAX_PROCESSES * MAX_FILES_PER_PROCESS;
const FIRST_DESCRIPTOR: u64 = 3;

#[derive(Clone, Copy)]
struct OpenFile {
    owner: ProcessId,
    node: Node<'static>,
    offset: usize,
}

struct FileTable {
    slots: [Option<OpenFile>; MAX_OPEN_FILES],
}

impl FileTable {
    const fn new() -> Self {
        Self {
            slots: [None; MAX_OPEN_FILES],
        }
    }

    fn open(&mut self, owner: ProcessId, node: Node<'static>) -> Result<u64, FileError> {
        if node.kind() != FileKind::Regular {
            return Err(FileError::IsDirectory);
        }
        let base = owner.slot() * MAX_FILES_PER_PROCESS;
        let range = base..base + MAX_FILES_PER_PROCESS;
        let (index, slot) = self
            .slots
            .iter_mut()
            .enumerate()
            .skip(range.start)
            .take(MAX_FILES_PER_PROCESS)
            .find(|(_, slot)| slot.is_none())
            .ok_or(FileError::TableFull)?;
        *slot = Some(OpenFile {
            owner,
            node,
            offset: 0,
        });
        FIRST_DESCRIPTOR
            .checked_add((index - base) as u64)
            .ok_or(FileError::BadDescriptor)
    }

    fn slot_mut(&mut self, owner: ProcessId, descriptor: u64) -> Result<&mut OpenFile, FileError> {
        let local_index = descriptor
            .checked_sub(FIRST_DESCRIPTOR)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < MAX_FILES_PER_PROCESS)
            .ok_or(FileError::BadDescriptor)?;
        let index = owner.slot() * MAX_FILES_PER_PROCESS + local_index;
        let file = self.slots[index]
            .as_mut()
            .filter(|file| file.owner == owner)
            .ok_or(FileError::BadDescriptor)?;
        Ok(file)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileError {
    BadDescriptor,
    IsDirectory,
    TableFull,
    Filesystem(FsError),
}

static FILES: Mutex<FileTable> = Mutex::new(FileTable::new());

pub fn open(owner: ProcessId, path: &str) -> Result<u64, FileError> {
    let node = crate::fs::vfs::resolve(path).map_err(FileError::Filesystem)?;
    if node.kind() != FileKind::Regular {
        return Err(FileError::IsDirectory);
    }
    x86_64::instructions::interrupts::without_interrupts(|| FILES.lock().open(owner, node))
}

pub fn read_plan(owner: ProcessId, descriptor: u64) -> Result<(Node<'static>, usize), FileError> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut files = FILES.lock();
        let file = files.slot_mut(owner, descriptor)?;
        Ok((file.node, file.offset))
    })
}

pub fn commit_read(
    owner: ProcessId,
    descriptor: u64,
    old_offset: usize,
    count: usize,
) -> Result<(), FileError> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut files = FILES.lock();
        let file = files.slot_mut(owner, descriptor)?;
        if file.offset != old_offset {
            return Err(FileError::BadDescriptor);
        }
        file.offset = old_offset
            .checked_add(count)
            .ok_or(FileError::BadDescriptor)?;
        Ok(())
    })
}

pub fn close(owner: ProcessId, descriptor: u64) -> Result<(), FileError> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut files = FILES.lock();
        let local_index = descriptor
            .checked_sub(FIRST_DESCRIPTOR)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < MAX_FILES_PER_PROCESS)
            .ok_or(FileError::BadDescriptor)?;
        let index = owner.slot() * MAX_FILES_PER_PROCESS + local_index;
        if files.slots[index].is_none_or(|file| file.owner != owner) {
            return Err(FileError::BadDescriptor);
        }
        files.slots[index] = None;
        Ok(())
    })
}

pub fn close_all(owner: ProcessId) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut files = FILES.lock();
        for slot in &mut files.slots {
            if slot.is_some_and(|file| file.owner == owner) {
                *slot = None;
            }
        }
    });
}
