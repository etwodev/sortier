use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use sortier_library::library::ItemId;

use crate::client::{RemoteClient, RemoteError};

/// How much is fetched at a time.
const BLOCK: u64 = 1 << 20;

/// A file on the server read like a local one, a block at a time, as a video player reads it.
pub struct RemoteFile {
    client: Arc<RemoteClient>,
    item: ItemId,
    length: u64,
    position: u64,
    /// The block read last, and where it starts.
    block: Option<(u64, Vec<u8>)>,
}

impl RemoteFile {
    /// Item `item`'s file, reading its first block to learn its length.
    pub fn open(client: Arc<RemoteClient>, item: ItemId) -> Result<Self, RemoteError> {
        let (first, length) = client.file_range(item, 0, BLOCK)?;
        Ok(Self {
            client,
            item,
            length,
            position: 0,
            block: Some((0, first)),
        })
    }

    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
}

impl Read for RemoteFile {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.position >= self.length || buffer.is_empty() {
            return Ok(0);
        }
        let start = self.position / BLOCK * BLOCK;
        if self.block.as_ref().is_none_or(|(at, _)| *at != start) {
            let (bytes, _) = self.client.file_range(self.item, start, BLOCK).map_err(std::io::Error::other)?;
            self.block = Some((start, bytes));
        }
        let (at, bytes) = self.block.as_ref().expect("read above");
        let offset = (self.position - at) as usize;
        let available = bytes.len().saturating_sub(offset);
        if available == 0 {
            return Ok(0);
        }
        let count = available.min(buffer.len());
        buffer[..count].copy_from_slice(&bytes[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for RemoteFile {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let position = match to {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => self.length.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        };
        self.position = position.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        Ok(self.position)
    }
}
