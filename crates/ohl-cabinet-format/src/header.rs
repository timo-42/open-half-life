//! The validated view over a whole cabinet header buffer.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bytes::{Cursor, add};
use crate::common::CommonHeader;
use crate::descriptor::{CabDescriptor, MAX_FILE_GROUP_COUNT, MIN_CAB_DESCRIPTOR_SIZE};
use crate::error::{FormatError, Limit};
use crate::file::FileDescriptor;
use crate::limits::Limits;
use crate::strings;
use crate::tables::{Component, DirectoryIter, FileDescriptorIter, FileGroup};
use crate::version::{Layout, Version};

/// Encoded size of one offset-list node.
const OFFSET_LIST_SIZE: usize = 12;

/// Byte distance from a file-group descriptor's name offset to its first
/// file index, per layout.
const FILE_GROUP_SKIP_V5: usize = 0x48;
const FILE_GROUP_SKIP_V6: usize = 0x12;

/// Byte distance from a component descriptor's name offset to its file-group
/// count, per layout.
const COMPONENT_SKIP_V5: usize = 0x6c;
const COMPONENT_SKIP_V6: usize = 0x6b;

/// A parsed, fully bounds-checked cabinet header.
///
/// Every accessor validates the offsets it follows against both the supplied
/// buffer and the supplied [`Limits`]; no accessor can read out of bounds and
/// none allocates more than the limits allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CabinetHeader<'a> {
    data: &'a [u8],
    limits: Limits,
    common: CommonHeader,
    version: Version,
    cab: CabDescriptor,
    header_index: u16,
    file_table: Vec<u32>,
    file_groups: Vec<FileGroup>,
    components: Vec<Component>,
}

impl<'a> CabinetHeader<'a> {
    /// Parses `data` as a cabinet header, decoding the version from the
    /// common header.
    pub fn parse(data: &'a [u8], limits: &Limits) -> Result<Self, FormatError> {
        Self::parse_inner(data, limits, None, 1)
    }

    /// Parses `data` as a cabinet header, forcing `major` rather than
    /// decoding the version word.
    pub fn parse_forced_version(
        data: &'a [u8],
        limits: &Limits,
        major: u16,
    ) -> Result<Self, FormatError> {
        Self::parse_inner(data, limits, Some(major), 1)
    }

    /// Returns the header with its volume number for InstallShield 5
    /// descriptors set to `index` (the number of the `.hdr`/`.cab` file the
    /// buffer came from). Defaults to 1.
    #[must_use]
    pub fn with_header_index(mut self, index: u16) -> Self {
        self.header_index = index;
        self
    }

    fn parse_inner(
        data: &'a [u8],
        limits: &Limits,
        forced_major: Option<u16>,
        header_index: u16,
    ) -> Result<Self, FormatError> {
        if data.len() > limits.max_header_bytes {
            return Err(FormatError::LimitExceeded(Limit::HeaderBytes));
        }

        let common = CommonHeader::parse(data)?;
        let version = match forced_major {
            Some(major) => Version::forced(common.version, major),
            None => Version::decode(common.version),
        };

        if common.cab_descriptor_size == 0 {
            return Err(FormatError::Malformed);
        }
        let descriptor_start = common.cab_descriptor_offset as usize;
        let descriptor_end = add(descriptor_start, common.cab_descriptor_size as usize)?;
        if descriptor_end > data.len() {
            return Err(FormatError::OffsetOutOfRange);
        }
        if (common.cab_descriptor_size as usize) < MIN_CAB_DESCRIPTOR_SIZE {
            return Err(FormatError::Truncated);
        }
        let cab = CabDescriptor::parse(&data[descriptor_start..descriptor_end])?;

        if cab.directory_count > limits.max_directories {
            return Err(FormatError::LimitExceeded(Limit::Directories));
        }
        if cab.file_count > limits.max_files {
            return Err(FormatError::LimitExceeded(Limit::Files));
        }

        let mut header = Self {
            data,
            limits: *limits,
            common,
            version,
            cab,
            header_index,
            file_table: Vec::new(),
            file_groups: Vec::new(),
            components: Vec::new(),
        };

        header.file_table = header.parse_file_table()?;
        header.file_groups = header.parse_file_groups()?;
        header.components = header.parse_components()?;
        Ok(header)
    }

    /// The common header as stored.
    #[must_use]
    pub const fn common(&self) -> &CommonHeader {
        &self.common
    }

    /// The decoded version.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The cabinet descriptor as stored.
    #[must_use]
    pub const fn descriptor(&self) -> &CabDescriptor {
        &self.cab
    }

    /// The limits this header was parsed under.
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Whether header strings are UTF-16LE.
    #[must_use]
    pub const fn is_unicode(&self) -> bool {
        self.version.is_unicode()
    }

    /// Number of directory entries.
    #[must_use]
    pub const fn directory_count(&self) -> u32 {
        self.cab.directory_count
    }

    /// Number of file entries.
    #[must_use]
    pub const fn file_count(&self) -> u32 {
        self.cab.file_count
    }

    /// Absolute offset of `offset` bytes past the cabinet descriptor.
    ///
    /// A zero offset is the format's null and is rejected; the reference
    /// implementation instead returns an unchecked pointer, which is the
    /// unbounded read this crate exists to remove.
    fn descriptor_offset(&self, offset: u32) -> Result<usize, FormatError> {
        if offset == 0 {
            return Err(FormatError::OffsetOutOfRange);
        }
        let absolute = add(self.common.cab_descriptor_offset as usize, offset as usize)?;
        if absolute >= self.data.len() {
            return Err(FormatError::OffsetOutOfRange);
        }
        Ok(absolute)
    }

    /// Absolute offset of the file table.
    fn file_table_base(&self) -> Result<usize, FormatError> {
        let absolute = add(
            self.common.cab_descriptor_offset as usize,
            self.cab.file_table_offset as usize,
        )?;
        if absolute > self.data.len() {
            return Err(FormatError::OffsetOutOfRange);
        }
        Ok(absolute)
    }

    /// Absolute offset of `offset` bytes past the file table.
    fn file_table_offset(&self, offset: u32) -> Result<usize, FormatError> {
        let absolute = add(self.file_table_base()?, offset as usize)?;
        if absolute >= self.data.len() {
            return Err(FormatError::OffsetOutOfRange);
        }
        Ok(absolute)
    }

    fn parse_file_table(&self) -> Result<Vec<u32>, FormatError> {
        let count = u64::from(self.cab.directory_count) + u64::from(self.cab.file_count);
        let count = usize::try_from(count).map_err(|_| FormatError::Malformed)?;
        let mut cursor = Cursor::at(self.data, self.file_table_base()?)?;
        let mut table = Vec::new();
        table
            .try_reserve_exact(count)
            .map_err(|_| FormatError::LimitExceeded(Limit::Files))?;
        for _ in 0..count {
            table.push(cursor.u32()?);
        }
        Ok(table)
    }

    /// The raw file table: `directory_count` directory-name offsets followed
    /// by `file_count` InstallShield 5 descriptor offsets.
    #[must_use]
    pub fn file_table(&self) -> &[u32] {
        &self.file_table
    }

    /// Decodes a string stored `offset` bytes past the cabinet descriptor.
    pub fn descriptor_string(&self, offset: u32) -> Result<String, FormatError> {
        let absolute = self.descriptor_offset(offset)?;
        strings::decode(
            &self.data[absolute..],
            self.is_unicode(),
            self.limits.max_name_bytes,
        )
    }

    /// Decodes a string stored `offset` bytes past the file table.
    pub fn file_table_string(&self, offset: u32) -> Result<String, FormatError> {
        let absolute = self.file_table_offset(offset)?;
        strings::decode(
            &self.data[absolute..],
            self.is_unicode(),
            self.limits.max_name_bytes,
        )
    }

    /// Decodes the name of directory `index`.
    pub fn directory_name(&self, index: u32) -> Result<String, FormatError> {
        if index >= self.cab.directory_count {
            return Err(FormatError::IndexOutOfRange);
        }
        let entry = *self
            .file_table
            .get(index as usize)
            .ok_or(FormatError::IndexOutOfRange)?;
        self.file_table_string(entry)
    }

    /// A bounded iterator over every directory name.
    #[must_use]
    pub fn directories(&self) -> DirectoryIter<'_, 'a> {
        DirectoryIter::new(self)
    }

    /// Parses the descriptor of file `index`.
    pub fn file_descriptor(&self, index: u32) -> Result<FileDescriptor, FormatError> {
        if index >= self.cab.file_count {
            return Err(FormatError::IndexOutOfRange);
        }

        match self.version.layout() {
            Layout::V5 => {
                let slot = usize::try_from(u64::from(self.cab.directory_count) + u64::from(index))
                    .map_err(|_| FormatError::IndexOutOfRange)?;
                let entry = *self
                    .file_table
                    .get(slot)
                    .ok_or(FormatError::IndexOutOfRange)?;
                let mut cursor = Cursor::at(self.data, self.file_table_offset(entry)?)?;
                FileDescriptor::parse_v5(&mut cursor, self.version.major() == 5, self.header_index)
            }
            Layout::V6 => {
                let stride = crate::file::FILE_DESCRIPTOR_SIZE_V6;
                let scaled = (index as usize)
                    .checked_mul(stride)
                    .ok_or(FormatError::OffsetOutOfRange)?;
                let base = add(
                    self.file_table_base()?,
                    add(self.cab.file_table_offset2 as usize, scaled)?,
                )?;
                let mut cursor = Cursor::at(self.data, base)?;
                FileDescriptor::parse_v6(&mut cursor)
            }
        }
    }

    /// Decodes the name of file `index`.
    pub fn file_name(&self, index: u32) -> Result<String, FormatError> {
        let descriptor = self.file_descriptor(index)?;
        self.file_table_string(descriptor.name_offset)
    }

    /// A bounded iterator over every file descriptor.
    #[must_use]
    pub fn file_descriptors(&self) -> FileDescriptorIter<'_, 'a> {
        FileDescriptorIter::new(self)
    }

    /// Walks one fixed offset-list array, calling `parse_node` for each
    /// descriptor offset it reaches.
    fn walk_offset_lists<T, F>(
        &self,
        heads: &[u32],
        limit: u32,
        limit_kind: Limit,
        mut parse_node: F,
    ) -> Result<Vec<T>, FormatError>
    where
        F: FnMut(&Self, u32) -> Result<T, FormatError>,
    {
        let mut collected = Vec::new();
        for head in heads {
            let mut next = *head;
            let mut visited = BTreeSet::new();
            while next != 0 {
                if !visited.insert(next) {
                    return Err(FormatError::LinkCycle);
                }
                let count = u32::try_from(collected.len()).unwrap_or(u32::MAX);
                if count >= limit {
                    return Err(FormatError::LimitExceeded(limit_kind));
                }
                let node_start = self.descriptor_offset(next)?;
                if add(node_start, OFFSET_LIST_SIZE)? > self.data.len() {
                    return Err(FormatError::Truncated);
                }
                let mut cursor = Cursor::at(self.data, node_start)?;
                let _name_offset = cursor.u32()?;
                let descriptor_offset = cursor.u32()?;
                next = cursor.u32()?;
                collected.push(parse_node(self, descriptor_offset)?);
            }
        }
        Ok(collected)
    }

    fn parse_file_groups(&self) -> Result<Vec<FileGroup>, FormatError> {
        let heads = self.cab.file_group_offsets;
        self.walk_offset_lists(
            &heads,
            self.limits.max_file_groups,
            Limit::FileGroups,
            Self::parse_file_group,
        )
    }

    fn parse_file_group(&self, offset: u32) -> Result<FileGroup, FormatError> {
        let mut cursor = Cursor::at(self.data, self.descriptor_offset(offset)?)?;
        let name_offset = cursor.u32()?;
        // The reference implementation keys this skip on `major <= 5`, which
        // differs from the component skip below; both are reproduced exactly.
        let skip = if self.version.major() <= 5 {
            FILE_GROUP_SKIP_V5
        } else {
            FILE_GROUP_SKIP_V6
        };
        cursor.skip(skip)?;
        let first_file = cursor.u32()?;
        let last_file = cursor.u32()?;
        Ok(FileGroup {
            name: self.descriptor_string(name_offset)?,
            first_file,
            last_file,
        })
    }

    fn parse_components(&self) -> Result<Vec<Component>, FormatError> {
        let heads = self.cab.component_offsets;
        self.walk_offset_lists(
            &heads,
            self.limits.max_components,
            Limit::Components,
            Self::parse_component,
        )
    }

    fn parse_component(&self, offset: u32) -> Result<Component, FormatError> {
        let mut cursor = Cursor::at(self.data, self.descriptor_offset(offset)?)?;
        let name_offset = cursor.u32()?;
        let skip = match self.version.layout() {
            Layout::V5 => COMPONENT_SKIP_V5,
            Layout::V6 => COMPONENT_SKIP_V6,
        };
        cursor.skip(skip)?;
        let file_group_count = cursor.u16()?;
        // The reference implementation calls `abort()` here; a malformed
        // count is a parse failure, never a process exit.
        if file_group_count > MAX_FILE_GROUP_COUNT {
            return Err(FormatError::Malformed);
        }
        let file_group_table_offset = cursor.u32()?;

        let mut file_group_names = Vec::new();
        if file_group_count > 0 {
            let mut table =
                Cursor::at(self.data, self.descriptor_offset(file_group_table_offset)?)?;
            for _ in 0..file_group_count {
                let entry = table.u32()?;
                file_group_names.push(self.descriptor_string(entry)?);
            }
        }

        Ok(Component {
            name: self.descriptor_string(name_offset)?,
            file_group_names,
        })
    }

    /// Every parsed file group, in offset-list order.
    #[must_use]
    pub fn file_groups(&self) -> &[FileGroup] {
        &self.file_groups
    }

    /// The file group at `index`.
    #[must_use]
    pub fn file_group(&self, index: usize) -> Option<&FileGroup> {
        self.file_groups.get(index)
    }

    /// Every parsed component, in offset-list order.
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// The component at `index`.
    #[must_use]
    pub fn component(&self, index: usize) -> Option<&Component> {
        self.components.get(index)
    }
}
