//! Bounded volume registry and read-only VFS over filesystem providers.

use crate::path::{PathError, VfsPath, VolumeName};

pub const MAX_VOLUMES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    Regular,
    Directory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderNode {
    pub kind: FileKind,
    pub len: usize,
    pub handle: u64,
}

/// Filesystem-specific provider contract. Paths arrive as validated,
/// case-preserving components. Component matching semantics belong to each
/// provider; the generic VFS does not normalize component case.
pub trait FilesystemProvider: Sync {
    fn lookup(&self, components: &[&str]) -> Option<ProviderNode>;

    fn read(&self, node: ProviderNode, offset: usize, output: &mut [u8]) -> Result<usize, FsError>;

    /// Optional zero-copy view for memory-backed executable providers.
    fn contiguous_file(&self, _node: ProviderNode) -> Option<&[u8]> {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsError {
    NotFound,
    NotDirectory,
    IsDirectory,
    InvalidPath,
    InvalidVolumeName,
    InvalidOffset,
    VolumeNotFound,
    DuplicateVolume,
    VolumeTableFull,
    ProviderFailure,
    NotContiguous,
}

impl From<PathError> for FsError {
    fn from(error: PathError) -> Self {
        match error {
            PathError::InvalidVolumeName => Self::InvalidVolumeName,
            _ => Self::InvalidPath,
        }
    }
}

#[derive(Clone, Copy)]
struct Volume<'a> {
    name: VolumeName,
    filesystem: &'a dyn FilesystemProvider,
}

pub struct VolumeRegistry<'a> {
    volumes: [Option<Volume<'a>>; MAX_VOLUMES],
    len: usize,
}

impl<'a> VolumeRegistry<'a> {
    pub const fn new() -> Self {
        Self {
            volumes: [None; MAX_VOLUMES],
            len: 0,
        }
    }

    pub fn register(
        &mut self,
        name: &str,
        filesystem: &'a dyn FilesystemProvider,
    ) -> Result<(), FsError> {
        let name = VolumeName::parse(name).map_err(FsError::from)?;
        if self.volumes[..self.len]
            .iter()
            .flatten()
            .any(|volume| volume.name.matches(&name))
        {
            return Err(FsError::DuplicateVolume);
        }
        if self.len == MAX_VOLUMES {
            return Err(FsError::VolumeTableFull);
        }
        self.volumes[self.len] = Some(Volume { name, filesystem });
        self.len += 1;
        Ok(())
    }

    fn get(&self, name: &VolumeName) -> Option<&'a dyn FilesystemProvider> {
        self.volumes[..self.len]
            .iter()
            .flatten()
            .find(|volume| volume.name.matches(name))
            .map(|volume| volume.filesystem)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for VolumeRegistry<'_> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Vfs<'a> {
    volumes: VolumeRegistry<'a>,
}

impl<'a> Vfs<'a> {
    pub const fn new() -> Self {
        Self {
            volumes: VolumeRegistry::new(),
        }
    }

    pub fn register_volume(
        &mut self,
        name: &str,
        filesystem: &'a dyn FilesystemProvider,
    ) -> Result<(), FsError> {
        self.volumes.register(name, filesystem)
    }

    pub fn volume_count(&self) -> usize {
        self.volumes.len()
    }

    pub fn resolve(&self, path: &str) -> Result<Node<'_>, FsError> {
        let canonical = VfsPath::parse(path).map_err(FsError::from)?;
        let filesystem = self
            .volumes
            .get(canonical.volume())
            .ok_or(FsError::VolumeNotFound)?;
        let count = canonical.component_count();
        let mut components = [""; crate::path::MAX_PATH_COMPONENTS];
        for (index, component) in components.iter_mut().take(count).enumerate() {
            *component = canonical
                .component(index)
                .expect("component index is bounded");
        }

        let mut ancestor_count = count;
        while ancestor_count > 1 {
            ancestor_count -= 1;
            if let Some(ancestor) = filesystem.lookup(&components[..ancestor_count]) {
                if ancestor.kind != FileKind::Directory {
                    return Err(FsError::NotDirectory);
                }
            } else {
                return Err(FsError::NotFound);
            }
        }

        if count == 0 {
            return Ok(Node {
                path: canonical,
                kind: FileKind::Directory,
                filesystem,
                provider_node: ProviderNode {
                    kind: FileKind::Directory,
                    len: 0,
                    handle: 0,
                },
            });
        }
        let node = filesystem
            .lookup(&components[..count])
            .ok_or(FsError::NotFound)?;
        Ok(Node {
            path: canonical,
            kind: node.kind,
            filesystem,
            provider_node: node,
        })
    }
}

impl Default for Vfs<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub struct Node<'a> {
    path: VfsPath,
    kind: FileKind,
    filesystem: &'a dyn FilesystemProvider,
    provider_node: ProviderNode,
}

impl core::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Node")
            .field("path", &self.path.as_str())
            .field("kind", &self.kind)
            .field("len", &self.provider_node.len)
            .finish()
    }
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.kind == other.kind
            && self.provider_node == other.provider_node
    }
}

impl Eq for Node<'_> {}

impl<'a> Node<'a> {
    pub fn kind(self) -> FileKind {
        self.kind
    }

    pub fn len(self) -> usize {
        self.provider_node.len
    }

    pub fn is_empty(self) -> bool {
        self.provider_node.len == 0
    }

    pub fn read(self, offset: usize, output: &mut [u8]) -> Result<usize, FsError> {
        if self.kind == FileKind::Directory {
            return Err(FsError::IsDirectory);
        }
        if offset > self.provider_node.len {
            return Err(FsError::InvalidOffset);
        }
        let maximum = output.len().min(self.provider_node.len - offset);
        let count = self
            .filesystem
            .read(self.provider_node, offset, &mut output[..maximum])?;
        if count > maximum {
            return Err(FsError::ProviderFailure);
        }
        Ok(count)
    }

    /// Archive-backed providers can pass their immutable contiguous payload
    /// to the ELF loader without copying. This is not required of all providers.
    pub fn contiguous_bytes(self) -> Result<&'a [u8], FsError> {
        if self.kind == FileKind::Directory {
            return Err(FsError::IsDirectory);
        }
        self.filesystem
            .contiguous_file(self.provider_node)
            .filter(|bytes| bytes.len() == self.provider_node.len)
            .ok_or(FsError::NotContiguous)
    }

    pub fn path(&self) -> &str {
        self.path.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestProvider;

    impl FilesystemProvider for TestProvider {
        fn lookup(&self, components: &[&str]) -> Option<ProviderNode> {
            match components {
                ["pvOS"] | ["pvOS", "System"] => Some(ProviderNode {
                    kind: FileKind::Directory,
                    len: 0,
                    handle: 0,
                }),
                ["pvOS", "System", "Init"] => Some(ProviderNode {
                    kind: FileKind::Regular,
                    len: 8,
                    handle: 1,
                }),
                ["MiXeD"] => Some(ProviderNode {
                    kind: FileKind::Regular,
                    len: 1,
                    handle: 2,
                }),
                ["file"] | ["file", "child"] => Some(ProviderNode {
                    kind: FileKind::Regular,
                    len: 1,
                    handle: 4,
                }),
                ["orphan", "child"] => Some(ProviderNode {
                    kind: FileKind::Regular,
                    len: 3,
                    handle: 3,
                }),
                _ => None,
            }
        }

        fn read(
            &self,
            node: ProviderNode,
            offset: usize,
            output: &mut [u8],
        ) -> Result<usize, FsError> {
            let bytes: &[u8] = match node.handle {
                1 => b"ELF init",
                2 | 4 => b"x",
                3 => b"bad",
                _ => return Err(FsError::ProviderFailure),
            };
            let count = output.len().min(bytes.len().saturating_sub(offset));
            output[..count].copy_from_slice(&bytes[offset..offset + count]);
            Ok(count)
        }

        fn contiguous_file(&self, node: ProviderNode) -> Option<&[u8]> {
            match node.handle {
                1 => Some(b"ELF init"),
                2 | 4 => Some(b"x"),
                3 => Some(b"bad"),
                _ => None,
            }
        }
    }

    #[test]
    fn boot_volume_resolves_and_reads_files() {
        let provider = TestProvider;
        let mut vfs = Vfs::new();
        vfs.register_volume("Boot", &provider).unwrap();
        assert_eq!(vfs.volume_count(), 1);
        let init = vfs.resolve("Boot:\\pvOS\\System\\Init").unwrap();
        assert!(vfs.resolve("boot:\\pvOS\\System\\Init").is_ok());
        assert!(vfs.resolve("BOOT:\\pvOS\\System\\Init").is_ok());
        assert_eq!(init.kind(), FileKind::Regular);
        assert_eq!(init.path(), "Boot:\\pvOS\\System\\Init");
        let mut output = [0; 3];
        assert_eq!(init.read(1, &mut output), Ok(3));
        assert_eq!(&output, b"LF ");
        assert_eq!(init.read(init.len(), &mut output), Ok(0));
        assert_eq!(
            init.read(init.len() + 1, &mut output),
            Err(FsError::InvalidOffset)
        );
    }

    #[test]
    fn generic_vfs_preserves_component_case_for_provider_matching() {
        let provider = TestProvider;
        let mut vfs = Vfs::new();
        vfs.register_volume("Boot", &provider).unwrap();

        assert!(vfs.resolve("Boot:\\MiXeD").is_ok());
        assert_eq!(vfs.resolve("Boot:\\mIxEd"), Err(FsError::NotFound));
    }

    #[test]
    fn volume_matching_and_registration_errors_are_deterministic() {
        let provider = TestProvider;
        let mut vfs = Vfs::new();
        assert!(vfs.resolve("C:\\pvOS").is_err());
        vfs.register_volume("Boot", &provider).unwrap();
        assert!(vfs.resolve("bOoT:\\pvOS").is_ok());
        assert_eq!(
            vfs.register_volume("bOOT", &provider),
            Err(FsError::DuplicateVolume)
        );
        assert_eq!(
            vfs.register_volume("bad name", &provider),
            Err(FsError::InvalidVolumeName)
        );
        assert_eq!(vfs.resolve("C:\\pvOS"), Err(FsError::VolumeNotFound));
    }

    #[test]
    fn directory_traversal_and_capacity_are_enforced() {
        let provider = TestProvider;
        let mut vfs = Vfs::new();
        vfs.register_volume("Boot", &provider).unwrap();
        assert_eq!(vfs.resolve("Boot:\\").unwrap().kind(), FileKind::Directory);
        assert_eq!(
            vfs.resolve("Boot:\\pvOS\\System").unwrap().kind(),
            FileKind::Directory
        );
        assert_eq!(vfs.resolve("Boot:\\missing"), Err(FsError::NotFound));
        assert_eq!(
            vfs.resolve("Boot:\\file\\child"),
            Err(FsError::NotDirectory)
        );
        assert_eq!(vfs.resolve("Boot:\\orphan\\child"), Err(FsError::NotFound));
        let mut registry = VolumeRegistry::new();
        for index in 0..MAX_VOLUMES {
            let mut name = [b'A'; 4];
            name[1] = b'0' + index as u8;
            let name = core::str::from_utf8(&name).unwrap();
            registry.register(name, &provider).unwrap();
        }
        assert_eq!(
            registry.register("Overflow", &provider),
            Err(FsError::VolumeTableFull)
        );
    }

    #[test]
    fn directory_read_is_rejected() {
        let provider = TestProvider;
        let mut vfs = Vfs::new();
        vfs.register_volume("Boot", &provider).unwrap();
        assert_eq!(
            vfs.resolve("Boot:\\pvOS").unwrap().read(0, &mut [0; 1]),
            Err(FsError::IsDirectory)
        );
    }
}
