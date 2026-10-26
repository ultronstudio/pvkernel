//! Checked, allocation-free parser for the `070701` CPIO newc subset.

const HEADER_LEN: usize = 110;
const NAME_MAX: usize = 255;
const ENTRY_MAX: usize = 128;
const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpioError {
    TruncatedHeader,
    BadMagic,
    InvalidHex,
    IntegerOverflow,
    InvalidNameSize,
    NameOutOfBounds,
    MissingNameNul,
    InvalidPath,
    InvalidPadding,
    UnsupportedChecksum,
    DataOutOfBounds,
    UnsupportedType,
    DuplicatePath,
    MissingTrailer,
    TooManyEntries,
    NonZeroTail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    Regular,
    Directory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry<'a> {
    pub kind: EntryKind,
    pub data: &'a [u8],
    pub data_offset: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Archive<'a> {
    bytes: &'a [u8],
    end: usize,
}

impl<'a> Archive<'a> {
    /// Validates the full archive before returning a view. No allocation or
    /// unaligned struct access is used; archive metadata is treated as hostile.
    pub fn validate(bytes: &'a [u8]) -> Result<Self, CpioError> {
        let mut offset = 0usize;
        let mut entries = 0usize;
        loop {
            if offset == bytes.len() {
                return Err(CpioError::MissingTrailer);
            }
            let entry = parse_entry(bytes, offset)?;
            if entry.name == "TRAILER!!!" {
                if !entry.data.is_empty() {
                    return Err(CpioError::InvalidNameSize);
                }
                if bytes[entry.end..].iter().any(|byte| *byte != 0) {
                    return Err(CpioError::NonZeroTail);
                }
                return Ok(Self {
                    bytes,
                    end: entry.end,
                });
            }
            entries = entries.checked_add(1).ok_or(CpioError::TooManyEntries)?;
            if entries > ENTRY_MAX {
                return Err(CpioError::TooManyEntries);
            }
            // A second bounded pass rejects duplicate raw/canonical names.
            let mut previous = 0usize;
            while previous < offset {
                let prior = parse_entry(bytes, previous)?;
                if prior.name.as_bytes() == entry.name.as_bytes() {
                    return Err(CpioError::DuplicatePath);
                }
                previous = prior.end;
            }
            offset = entry.end;
        }
    }

    pub fn find(&self, name: &[u8]) -> Result<Option<Entry<'a>>, CpioError> {
        let mut offset = 0;
        loop {
            let entry = parse_entry(self.bytes, offset)?;
            if entry.name == "TRAILER!!!" {
                return Ok(None);
            }
            if entry.name.as_bytes() == name {
                return Ok(Some(entry.public()));
            }
            offset = entry.end;
        }
    }

    /// Returns a validated archive-backed data range for a provider handle.
    pub fn data_at(&self, offset: usize, len: usize) -> Option<&'a [u8]> {
        let end = offset.checked_add(len)?;
        self.bytes.get(offset..end)
    }

    pub fn entry_count(&self) -> usize {
        let mut offset = 0;
        let mut count = 0;
        while offset < self.end {
            let Ok(entry) = parse_entry(self.bytes, offset) else {
                return 0;
            };
            if entry.name == "TRAILER!!!" {
                break;
            }
            count += 1;
            offset = entry.end;
        }
        count
    }
}

struct Parsed<'a> {
    name: &'a str,
    kind: EntryKind,
    data: &'a [u8],
    data_offset: usize,
    end: usize,
}
impl<'a> Parsed<'a> {
    fn public(self) -> Entry<'a> {
        Entry {
            kind: self.kind,
            data: self.data,
            data_offset: self.data_offset,
        }
    }
}

fn parse_entry(bytes: &[u8], offset: usize) -> Result<Parsed<'_>, CpioError> {
    let header_end = offset
        .checked_add(HEADER_LEN)
        .ok_or(CpioError::IntegerOverflow)?;
    let header = bytes
        .get(offset..header_end)
        .ok_or(CpioError::TruncatedHeader)?;
    if header.get(0..6) != Some(b"070701") {
        return Err(CpioError::BadMagic);
    }
    // Decode every numeric header field, including metadata not currently
    // exposed by the VFS, so malformed hexadecimal cannot hide in ignored
    // fields and later acquire meaning if the supported subset grows.
    let mut fields = [0u32; 13];
    for (index, value) in fields.iter_mut().enumerate() {
        *value = field(header, 6 + index * 8)?;
    }
    let mode = fields[1];
    let link_count = fields[4];
    let file_size = usize::try_from(fields[6]).map_err(|_| CpioError::IntegerOverflow)?;
    let name_size = usize::try_from(fields[11]).map_err(|_| CpioError::IntegerOverflow)?;
    if fields[12] != 0 {
        return Err(CpioError::UnsupportedChecksum);
    }
    if name_size == 0 || name_size > NAME_MAX + 1 {
        return Err(CpioError::InvalidNameSize);
    }
    let name_start = header_end;
    let name_end = name_start
        .checked_add(name_size)
        .ok_or(CpioError::IntegerOverflow)?;
    let name_region = bytes
        .get(name_start..name_end)
        .ok_or(CpioError::NameOutOfBounds)?;
    if name_region.last() != Some(&0) || name_region[..name_region.len() - 1].contains(&0) {
        return Err(CpioError::MissingNameNul);
    }
    let name = core::str::from_utf8(&name_region[..name_region.len() - 1])
        .map_err(|_| CpioError::InvalidPath)?;
    let data_start = align4(name_end)?;
    if bytes
        .get(name_end..data_start)
        .ok_or(CpioError::InvalidPadding)?
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(CpioError::InvalidPadding);
    }
    let data_end = data_start
        .checked_add(file_size)
        .ok_or(CpioError::IntegerOverflow)?;
    let data = bytes
        .get(data_start..data_end)
        .ok_or(CpioError::DataOutOfBounds)?;
    let end = align4(data_end)?;
    if bytes
        .get(data_end..end)
        .ok_or(CpioError::InvalidPadding)?
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(CpioError::InvalidPadding);
    }
    let kind = if name == "TRAILER!!!" {
        EntryKind::Directory
    } else {
        match mode & S_IFMT {
            S_IFREG if link_count == 1 => EntryKind::Regular,
            S_IFDIR if file_size == 0 => EntryKind::Directory,
            _ => return Err(CpioError::UnsupportedType),
        }
    };
    if name != "TRAILER!!!" {
        validate_archive_name(name)?;
    }
    Ok(Parsed {
        name,
        kind,
        data,
        data_offset: data_start,
        end,
    })
}

fn validate_archive_name(name: &str) -> Result<(), CpioError> {
    if name.is_empty() || name.starts_with('/') || name.len() > NAME_MAX {
        return Err(CpioError::InvalidPath);
    }
    for component in name.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > 63
            || component.as_bytes().contains(&0)
            || component
                .bytes()
                .any(|byte| byte == b'\\' || byte == b':' || byte.is_ascii_control())
        {
            return Err(CpioError::InvalidPath);
        }
    }
    Ok(())
}

fn field(header: &[u8], offset: usize) -> Result<u32, CpioError> {
    let end = offset.checked_add(8).ok_or(CpioError::IntegerOverflow)?;
    let bytes = header.get(offset..end).ok_or(CpioError::TruncatedHeader)?;
    let mut value = 0u32;
    for byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return Err(CpioError::InvalidHex),
        };
        value = value
            .checked_mul(16)
            .and_then(|v| v.checked_add(u32::from(digit)))
            .ok_or(CpioError::IntegerOverflow)?;
    }
    Ok(value)
}

fn align4(value: usize) -> Result<usize, CpioError> {
    value
        .checked_add(3)
        .map(|v| v & !3)
        .ok_or(CpioError::IntegerOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn append(output: &mut allocless::Bytes, name: &[u8], mode: u32, data: &[u8]) {
        output.extend_from_slice(b"070701");
        let fields = [
            1,
            mode,
            0,
            0,
            1,
            0,
            data.len() as u32,
            0,
            0,
            0,
            0,
            name.len() as u32 + 1,
            0,
        ];
        for field in fields {
            output.extend_from_slice(format!("{field:08x}").as_bytes());
        }
        output.extend_from_slice(name);
        output.push(0);
        while output.len() % 4 != 0 {
            output.push(0);
        }
        output.extend_from_slice(data);
        while output.len() % 4 != 0 {
            output.push(0);
        }
    }

    fn archive(entries: &[(&[u8], u32, &[u8])], trailer: bool) -> allocless::Bytes {
        let mut bytes = allocless::Bytes::new();
        for (name, mode, data) in entries {
            append(&mut bytes, name, *mode, data);
        }
        if trailer {
            append(&mut bytes, b"TRAILER!!!", 0, &[]);
        }
        bytes
    }

    // The test-only byte buffer keeps these tests usable from the no_std
    // source module without pulling an allocator into the kernel build.
    mod allocless {
        pub type Bytes = std::vec::Vec<u8>;
    }

    #[test]
    fn accepts_regular_and_directory_entries() {
        let bytes = archive(
            &[(b"system", S_IFDIR, &[]), (b"system/init", S_IFREG, b"ELF")],
            true,
        );
        let parsed = Archive::validate(&bytes).unwrap();
        assert_eq!(parsed.entry_count(), 2);
        assert_eq!(parsed.find(b"system/init").unwrap().unwrap().data, b"ELF");
        assert_eq!(parsed.find(b"missing").unwrap(), None);
    }

    #[test]
    fn rejects_truncation_and_bad_magic() {
        assert_eq!(
            Archive::validate(&[0; 20]).unwrap_err(),
            CpioError::TruncatedHeader
        );
        let mut bytes = [0; HEADER_LEN];
        bytes[..6].copy_from_slice(b"xxxxxx");
        assert_eq!(Archive::validate(&bytes).unwrap_err(), CpioError::BadMagic);
    }

    #[test]
    fn rejects_invalid_hex_and_out_of_bounds_sizes() {
        let mut bytes = archive(&[(b"x", S_IFREG, b"a")], true);
        bytes[14] = b'g';
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::InvalidHex
        );

        let mut bytes = archive(&[(b"x", S_IFREG, b"")], true);
        bytes[54..62].copy_from_slice(b"ffffffff");
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::DataOutOfBounds
        );

        let mut bytes = archive(&[(b"x", S_IFREG, b"")], false);
        bytes[94..102].copy_from_slice(b"00000020");
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::NameOutOfBounds
        );

        let mut bytes = archive(&[(b"x", S_IFREG, b"")], true);
        // Even a currently unused metadata field is decoded and validated.
        bytes[6] = b'g';
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::InvalidHex
        );
    }

    #[test]
    fn rejects_bad_terminator_padding_and_missing_trailer() {
        let mut bytes = archive(&[(b"x", S_IFREG, b"")], true);
        bytes[HEADER_LEN + 1] = 1;
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::MissingNameNul
        );

        let mut bytes = archive(&[(b"xy", S_IFREG, b"")], true);
        bytes[HEADER_LEN + 3] = 9;
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::InvalidPadding
        );

        let mut bytes = archive(&[(b"x", S_IFREG, b"a")], true);
        let data_padding = HEADER_LEN + 4 + 1;
        bytes[data_padding] = 9;
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::InvalidPadding
        );

        let bytes = archive(&[(b"x", S_IFREG, b"")], false);
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::MissingTrailer
        );
    }

    #[test]
    fn rejects_unsupported_types_and_duplicate_paths() {
        let bytes = archive(&[(b"x", 0o120000, b"target")], true);
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::UnsupportedType
        );
        let bytes = archive(&[(b"x", S_IFREG, b"a"), (b"x", S_IFREG, b"b")], true);
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::DuplicatePath
        );
    }

    #[test]
    fn rejects_noncanonical_and_overlong_archive_paths() {
        for name in [
            b"/absolute".as_slice(),
            b"../escape",
            b"bin//init",
            b"bin/./init",
            b"pvOS/Boot:\\Init",
            b"pvOS/Bad\\Name",
            b"pvOS/Bad\x1fName",
        ] {
            let bytes = archive(&[(name, S_IFREG, b"x")], true);
            assert_eq!(
                Archive::validate(&bytes).unwrap_err(),
                CpioError::InvalidPath
            );
        }
        let long_component = allocless::Bytes::from_iter(
            core::iter::once(b'a').chain(core::iter::repeat_n(b'x', 63)),
        );
        let bytes = archive(&[(&long_component, S_IFREG, b"x")], true);
        assert_eq!(
            Archive::validate(&bytes).unwrap_err(),
            CpioError::InvalidPath
        );
    }
}
