//! Read-only CPIO `newc` filesystem provider for the boot volume.

use crate::cpio::{Archive, EntryKind};
use crate::path::MAX_PATH_LEN;
use crate::vfs::{FileKind, FilesystemProvider, FsError, ProviderNode};

pub struct InitrdFs<'a> {
    archive: Archive<'a>,
}

impl<'a> InitrdFs<'a> {
    pub const fn new(archive: Archive<'a>) -> Self {
        Self { archive }
    }
}

impl FilesystemProvider for InitrdFs<'_> {
    fn lookup(&self, components: &[&str]) -> Option<ProviderNode> {
        if components.is_empty() {
            return Some(ProviderNode {
                kind: FileKind::Directory,
                len: 0,
                handle: 0,
            });
        }
        let mut canonical_name = [0; MAX_PATH_LEN];
        let mut len = 0usize;
        for component in components {
            if len != 0 {
                *canonical_name.get_mut(len)? = b'/';
                len += 1;
            }
            let end = len.checked_add(component.len())?;
            canonical_name
                .get_mut(len..end)?
                .copy_from_slice(component.as_bytes());
            len = end;
        }
        let entry = self.archive.find(&canonical_name[..len]).ok().flatten()?;
        Some(ProviderNode {
            kind: match entry.kind {
                EntryKind::Regular => FileKind::Regular,
                EntryKind::Directory => FileKind::Directory,
            },
            len: entry.data.len(),
            handle: u64::try_from(entry.data_offset).ok()?,
        })
    }

    fn read(&self, node: ProviderNode, offset: usize, output: &mut [u8]) -> Result<usize, FsError> {
        if node.kind == FileKind::Directory {
            return Err(FsError::IsDirectory);
        }
        if offset > node.len {
            return Err(FsError::InvalidOffset);
        }
        let archive_offset = usize::try_from(node.handle).map_err(|_| FsError::ProviderFailure)?;
        let bytes = self
            .archive
            .data_at(archive_offset, node.len)
            .ok_or(FsError::ProviderFailure)?;
        let count = output.len().min(node.len - offset);
        output[..count].copy_from_slice(&bytes[offset..offset + count]);
        Ok(count)
    }

    fn contiguous_file(&self, node: ProviderNode) -> Option<&[u8]> {
        if node.kind != FileKind::Regular {
            return None;
        }
        let offset = usize::try_from(node.handle).ok()?;
        self.archive.data_at(offset, node.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{FsError, Vfs};

    #[test]
    fn cpio_initrd_resolves_the_boot_volume_init_path() {
        let archive_bytes = archive();
        let archive = Archive::validate(&archive_bytes).unwrap();
        let initrd = InitrdFs::new(archive);
        let mut vfs = Vfs::new();
        vfs.register_volume("Boot", &initrd).unwrap();
        let init = vfs.resolve("Boot:\\pvOS\\System\\Init").unwrap();
        assert!(vfs.resolve("boot:\\pvOS\\System\\Init").is_ok());
        assert_eq!(
            vfs.resolve("Boot:\\pvos\\System\\Init"),
            Err(FsError::NotFound)
        );
        assert_eq!(
            vfs.resolve("Boot:\\pvOS\\system\\Init"),
            Err(FsError::NotFound)
        );
        assert_eq!(init.kind(), FileKind::Regular);
        assert_eq!(init.contiguous_bytes().unwrap(), b"ELF init");
        let mut partial = [0; 3];
        assert_eq!(init.read(1, &mut partial), Ok(3));
        assert_eq!(&partial, b"LF ");
        assert_eq!(
            vfs.resolve("Boot:\\pvOS\\System\\Missing"),
            Err(FsError::NotFound)
        );
    }

    fn archive() -> std::vec::Vec<u8> {
        let mut bytes = std::vec::Vec::new();
        append(&mut bytes, 1, b"pvOS", 0o040755, b"");
        append(&mut bytes, 2, b"pvOS/System", 0o040755, b"");
        append(&mut bytes, 3, b"pvOS/System/Init", 0o100755, b"ELF init");
        append(&mut bytes, 4, b"TRAILER!!!", 0, b"");
        bytes
    }

    fn append(bytes: &mut std::vec::Vec<u8>, ino: u32, name: &[u8], mode: u32, data: &[u8]) {
        bytes.extend_from_slice(b"070701");
        for field in [
            ino,
            mode,
            0,
            0,
            if mode & 0o170000 == 0o040000 { 2 } else { 1 },
            0,
            data.len() as u32,
            0,
            0,
            0,
            0,
            name.len() as u32 + 1,
            0,
        ] {
            bytes.extend_from_slice(format!("{field:08x}").as_bytes());
        }
        bytes.extend_from_slice(name);
        bytes.push(0);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(data);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }
}
