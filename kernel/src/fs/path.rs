//! Bounded, volume-qualified pvOS paths. Canonical paths use `Volume:\\...`.
//! Component spelling is preserved; filesystem providers define matching.

pub const MAX_PATH_LEN: usize = 255;
pub const MAX_VOLUME_NAME_LEN: usize = 16;
pub const MAX_COMPONENT_LEN: usize = 63;
pub const MAX_PATH_COMPONENTS: usize = 124;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathError {
    Empty,
    TooLong,
    VolumeMissing,
    InvalidVolumeName,
    MissingVolumeRootSeparator,
    ComponentTooLong,
    EmbeddedNul,
    InvalidCharacter,
    EscapesVolumeRoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VolumeName {
    bytes: [u8; MAX_VOLUME_NAME_LEN],
    len: u8,
}

impl VolumeName {
    pub fn parse(input: &str) -> Result<Self, PathError> {
        let bytes = input.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_VOLUME_NAME_LEN {
            return Err(PathError::InvalidVolumeName);
        }
        if !bytes[0].is_ascii_alphabetic()
            || !bytes[1..]
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(PathError::InvalidVolumeName);
        }
        let mut name = Self {
            bytes: [0; MAX_VOLUME_NAME_LEN],
            len: bytes.len() as u8,
        };
        name.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(name)
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: `parse` accepts only ASCII volume-name bytes.
        core::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("validated volume name is ASCII")
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.as_str().eq_ignore_ascii_case(other.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VfsPath {
    volume: VolumeName,
    bytes: [u8; MAX_PATH_LEN],
    len: u8,
    component_starts: [u8; MAX_PATH_COMPONENTS],
    component_lens: [u8; MAX_PATH_COMPONENTS],
    component_count: u8,
}

impl VfsPath {
    pub fn parse(input: &str) -> Result<Self, PathError> {
        let input_bytes = input.as_bytes();
        if input_bytes.is_empty() {
            return Err(PathError::Empty);
        }
        if input_bytes.len() > MAX_PATH_LEN {
            return Err(PathError::TooLong);
        }
        if input_bytes.contains(&0) {
            return Err(PathError::EmbeddedNul);
        }
        let colon = input.find(':').ok_or(PathError::VolumeMissing)?;
        let volume = VolumeName::parse(&input[..colon])?;
        let remainder = &input[colon + 1..];
        let mut separators = remainder.bytes();
        if !matches!(separators.next(), Some(b'\\' | b'/')) {
            return Err(PathError::MissingVolumeRootSeparator);
        }

        let mut path = Self {
            volume,
            bytes: [0; MAX_PATH_LEN],
            len: 0,
            component_starts: [0; MAX_PATH_COMPONENTS],
            component_lens: [0; MAX_PATH_COMPONENTS],
            component_count: 0,
        };
        let volume_bytes = volume.as_str().as_bytes();
        path.bytes[..volume_bytes.len()].copy_from_slice(volume_bytes);
        path.len = volume_bytes.len() as u8;
        path.bytes[usize::from(path.len)] = b':';
        path.bytes[usize::from(path.len) + 1] = b'\\';
        path.len += 2;

        for component in remainder.split(['\\', '/']) {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                if path.component_count == 0 {
                    return Err(PathError::EscapesVolumeRoot);
                }
                path.len = path.component_starts[usize::from(path.component_count) - 1];
                path.component_count -= 1;
                // The stored component start follows its separator. Remove
                // that separator too when a parent component is popped so
                // equivalent normalized paths serialize identically.
                if path.component_count != 0 {
                    path.len -= 1;
                }
                continue;
            }
            if component.len() > MAX_COMPONENT_LEN {
                return Err(PathError::ComponentTooLong);
            }
            if component
                .bytes()
                .any(|byte| byte == b':' || byte.is_ascii_control())
            {
                return Err(PathError::InvalidCharacter);
            }
            let count = usize::from(path.component_count);
            if count == MAX_PATH_COMPONENTS {
                return Err(PathError::TooLong);
            }
            if count != 0 {
                path.bytes[usize::from(path.len)] = b'\\';
                path.len += 1;
            }
            let start = usize::from(path.len);
            let end = start
                .checked_add(component.len())
                .filter(|end| *end <= MAX_PATH_LEN)
                .ok_or(PathError::TooLong)?;
            path.bytes[start..end].copy_from_slice(component.as_bytes());
            path.component_starts[count] = start as u8;
            path.component_lens[count] = component.len() as u8;
            path.component_count += 1;
            path.len = end as u8;
        }
        Ok(path)
    }

    pub fn volume(&self) -> &VolumeName {
        &self.volume
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: input is UTF-8, and canonicalization copies only whole
        // components plus the ASCII volume/separator bytes.
        core::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("canonical path remains UTF-8")
    }

    pub fn component_count(&self) -> usize {
        usize::from(self.component_count)
    }

    pub fn component(&self, index: usize) -> Option<&str> {
        if index >= self.component_count() {
            return None;
        }
        let start = usize::from(self.component_starts[index]);
        let end = start + usize::from(self.component_lens[index]);
        Some(
            core::str::from_utf8(&self.bytes[start..end])
                .expect("canonical path component remains UTF-8"),
        )
    }

    pub fn volume_root(&self) -> bool {
        self.component_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_volume_paths_and_separator_aliases() {
        assert_eq!(VfsPath::parse("Boot:\\").unwrap().as_str(), "Boot:\\");
        assert_eq!(
            VfsPath::parse("Boot:\\pvOS\\System\\Init")
                .unwrap()
                .as_str(),
            "Boot:\\pvOS\\System\\Init"
        );
        assert_eq!(
            VfsPath::parse("C:/pvOS//./System/").unwrap().as_str(),
            "C:\\pvOS\\System"
        );
        assert_eq!(
            VfsPath::parse("Boot:\\pvOS\\..\\System").unwrap().as_str(),
            "Boot:\\System"
        );
        assert_eq!(
            VfsPath::parse("Boot:\\pvOS\\System\\..").unwrap().as_str(),
            "Boot:\\pvOS"
        );
        assert_eq!(
            VfsPath::parse("Boot:\\pvOS\\..").unwrap().as_str(),
            "Boot:\\"
        );
        assert!(VfsPath::parse("Boot:\\").unwrap().volume_root());
    }

    #[test]
    fn rejects_invalid_volume_paths_and_traversal() {
        for (input, expected) in [
            ("", PathError::Empty),
            (":\\foo", PathError::InvalidVolumeName),
            ("Boot:", PathError::MissingVolumeRootSeparator),
            ("Boot:relative", PathError::MissingVolumeRootSeparator),
            ("Boot:\\..\\foo", PathError::EscapesVolumeRoot),
            ("Boot:\\a\\..\\..", PathError::EscapesVolumeRoot),
            ("Boot:\\a\0b", PathError::EmbeddedNul),
            ("bad volume:\\x", PathError::InvalidVolumeName),
            ("Boot:\\bad:name", PathError::InvalidCharacter),
            ("Boot:\\bad\u{7f}name", PathError::InvalidCharacter),
        ] {
            assert_eq!(VfsPath::parse(input), Err(expected), "{input:?}");
        }
        assert_eq!(
            VolumeName::parse("123").unwrap_err(),
            PathError::InvalidVolumeName
        );
        assert_eq!(
            VolumeName::parse("BootBootBootBootB").unwrap_err(),
            PathError::InvalidVolumeName
        );
        assert_eq!(
            VfsPath::parse(&format!("Boot:\\{}", "x".repeat(64))).unwrap_err(),
            PathError::ComponentTooLong
        );
        assert_eq!(
            VfsPath::parse(&format!("Boot:\\{}", "x".repeat(MAX_PATH_LEN))).unwrap_err(),
            PathError::TooLong
        );
    }

    #[test]
    fn volume_names_are_case_insensitive_and_component_spelling_is_preserved() {
        let path = VfsPath::parse("bOoT:\\pvOS\\System").unwrap();
        let boot = VolumeName::parse("Boot").unwrap();
        assert!(path.volume().matches(&boot));
        assert_eq!(path.component(0), Some("pvOS"));
        assert_eq!(path.component(1), Some("System"));
    }
}
