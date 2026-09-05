//! Component, file-group and directory tables, and their bounded iterators.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::FormatError;
use crate::file::FileDescriptor;
use crate::header::CabinetHeader;

/// One file group: a named, inclusive range of file indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileGroup {
    /// The group's decoded name.
    pub name: String,
    /// First file index in the group, as recorded.
    pub first_file: u32,
    /// Last file index in the group, as recorded.
    pub last_file: u32,
}

/// One component: a name and the file groups it selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// The component's decoded name.
    pub name: String,
    /// Names of the file groups the component selects.
    pub file_group_names: Vec<String>,
}

/// Bounded iterator over decoded directory names.
#[derive(Debug)]
pub struct DirectoryIter<'h, 'a> {
    header: &'h CabinetHeader<'a>,
    index: u32,
}

impl<'h, 'a> DirectoryIter<'h, 'a> {
    pub(crate) const fn new(header: &'h CabinetHeader<'a>) -> Self {
        Self { header, index: 0 }
    }
}

impl Iterator for DirectoryIter<'_, '_> {
    type Item = Result<String, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.header.directory_count() {
            return None;
        }
        let item = self.header.directory_name(self.index);
        self.index += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.header.directory_count().saturating_sub(self.index) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for DirectoryIter<'_, '_> {}

/// Bounded iterator over parsed file descriptors.
#[derive(Debug)]
pub struct FileDescriptorIter<'h, 'a> {
    header: &'h CabinetHeader<'a>,
    index: u32,
}

impl<'h, 'a> FileDescriptorIter<'h, 'a> {
    pub(crate) const fn new(header: &'h CabinetHeader<'a>) -> Self {
        Self { header, index: 0 }
    }
}

impl Iterator for FileDescriptorIter<'_, '_> {
    type Item = Result<FileDescriptor, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.header.file_count() {
            return None;
        }
        let item = self.header.file_descriptor(self.index);
        self.index += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.header.file_count().saturating_sub(self.index) as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for FileDescriptorIter<'_, '_> {}
