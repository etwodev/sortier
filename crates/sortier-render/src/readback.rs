use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::{Result, bail};

const IDLE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;
const FAILED: u8 = 3;

pub(crate) const BYTES_PER_PIXEL: u32 = 4;

/// A staging buffer frames are copied into. `state` is written by wgpu's `map_async` callback, which may
/// run on another thread, or after the ring is gone (the callback owns its own `Arc`, so that's harmless).
struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    revision: u64,
    sequence: u64,
}

/// A ring of staging buffers for copying frames of one size back to the CPU without stalling: several
/// frames can be in flight, and the newest finished one is taken.
///
/// Use [`ReadbackRing::copy`] while recording a frame, submit, then [`ReadbackRing::map`] the slot it
/// returned; [`ReadbackRing::take_newest`] after polling the device collects the result.
pub(crate) struct ReadbackRing {
    slots: Vec<Slot>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    padded_bytes_per_row: u32,
    next_sequence: u64,
}

impl ReadbackRing {
    pub(crate) fn new(device: &wgpu::Device, width: u32, height: u32, slots: usize) -> Self {
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let slots = (0..slots.max(1))
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: u64::from(padded_bytes_per_row) * u64::from(height),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(IDLE)),
                revision: 0,
                sequence: 0,
            })
            .collect();
        Self {
            slots,
            width,
            height,
            padded_bytes_per_row,
            next_sequence: 0,
        }
    }

    pub(crate) fn has_free_slot(&self) -> bool {
        self.slots.iter().any(|slot| slot.state.load(Ordering::Acquire) == IDLE)
    }

    /// Whether a frame was submitted that hasn't been taken yet, finished or not.
    pub(crate) fn has_untaken(&self) -> bool {
        self.slots.iter().any(|slot| slot.state.load(Ordering::Acquire) != IDLE)
    }

    /// Records a copy of `texture` into a free slot. Returns the slot index, or `None` if all are in
    /// flight. Call [`ReadbackRing::map`] with it after submitting.
    pub(crate) fn copy(&mut self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) -> Option<usize> {
        let index = self.slots.iter().position(|slot| slot.state.load(Ordering::Acquire) == IDLE)?;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.slots[index].buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        Some(index)
    }

    /// Starts mapping a slot whose copy has been submitted.
    pub(crate) fn map(&mut self, index: usize, revision: u64) {
        let slot = &mut self.slots[index];
        slot.revision = revision;
        slot.sequence = self.next_sequence;
        self.next_sequence += 1;
        slot.state.store(PENDING, Ordering::Release);
        let state = Arc::clone(&slot.state);
        slot.buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            state.store(if result.is_ok() { READY } else { FAILED }, Ordering::Release);
        });
    }

    /// The newest mapped frame as tightly packed rows with its revision, freeing every mapped slot
    /// (older finished frames are dropped unread). Errors if a mapping failed; the failed slots are
    /// freed so the ring keeps working.
    pub(crate) fn take_newest(&mut self) -> Result<Option<(Vec<u8>, u64)>> {
        if self.slots.iter().any(|slot| slot.state.load(Ordering::Acquire) == FAILED) {
            for slot in &self.slots {
                if slot.state.load(Ordering::Acquire) == FAILED {
                    slot.state.store(IDLE, Ordering::Release);
                }
            }
            bail!("copying a frame back from the GPU failed");
        }
        let newest = (0..self.slots.len())
            .filter(|index| self.slots[*index].state.load(Ordering::Acquire) == READY)
            .max_by_key(|index| self.slots[*index].sequence);
        let Some(newest) = newest else {
            return Ok(None);
        };
        let frame = {
            let slot = &self.slots[newest];
            let pixels = unpad_rows(&slot.buffer.get_mapped_range(..), self.padded_bytes_per_row, self.width, self.height);
            (pixels, slot.revision)
        };
        for slot in &self.slots {
            if slot.state.load(Ordering::Acquire) == READY {
                slot.buffer.unmap();
                slot.state.store(IDLE, Ordering::Release);
            }
        }
        Ok(Some(frame))
    }
}

/// Bytes per row of a buffer a `width`-pixel texture is copied into, which wgpu requires to be a
/// multiple of 256.
pub(crate) fn padded_bytes_per_row(width: u32) -> u32 {
    (width * BYTES_PER_PIXEL).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

/// Copies `height` rows of `width` pixels out of a buffer with `padded_bytes_per_row`-byte rows.
pub(crate) fn unpad_rows(mapped: &[u8], padded_bytes_per_row: u32, width: u32, height: u32) -> Vec<u8> {
    let row_bytes = (width * BYTES_PER_PIXEL) as usize;
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * padded_bytes_per_row as usize;
        pixels.extend_from_slice(&mapped[start..start + row_bytes]);
    }
    pixels
}
