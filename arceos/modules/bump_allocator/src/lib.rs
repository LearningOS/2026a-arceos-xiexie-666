#![no_std]

use allocator::{AllocError, AllocResult, BaseAllocator, ByteAllocator, PageAllocator};
use core::alloc::Layout;
use core::ptr::NonNull;

/// Early memory allocator
/// Use it before formal bytes-allocator and pages-allocator can work!
/// This is a double-end memory range:
/// - Alloc bytes forward
/// - Alloc pages backward
///
/// [ bytes-used | avail-area | pages-used ]
/// |            | -->    <-- |            |
/// start       b_pos        p_pos       end
///
/// For bytes area, 'count' records number of allocations.
/// When it goes down to ZERO, free bytes-used area.
/// For pages area, it will never be freed!
///
pub struct EarlyAllocator<const SIZE: usize> {
    start: usize,
    end: usize,
    b_pos: usize,
    p_pos: usize,
    count: usize,
}

impl<const SIZE: usize> EarlyAllocator<SIZE> {
    pub const fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            b_pos: 0,
            p_pos: 0,
            count: 0,
        }
    }
}

impl<const SIZE: usize> Default for EarlyAllocator<SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SIZE: usize> BaseAllocator for EarlyAllocator<SIZE> {
    fn init(&mut self, start: usize, size: usize) {
        self.start = start;
        self.end = start + size;
        self.b_pos = start;
        self.p_pos = self.end;
        self.count = 0;
    }

    fn add_memory(&mut self, _start: usize, _size: usize) -> AllocResult {
        // The early allocator works on a single contiguous region: byte
        // allocations grow from the bottom while page allocations grow from
        // the top, so another disjoint region cannot be merged into it.
        Err(AllocError::InvalidParam)
    }
}

impl<const SIZE: usize> ByteAllocator for EarlyAllocator<SIZE> {
    fn alloc(&mut self, layout: Layout) -> AllocResult<NonNull<u8>> {
        let align = layout.align();
        let size = layout.size();

        // The allocation starts at the first aligned address after the bytes
        // that are already used, so the bytes area only ever grows forward.
        let start = align_up(self.b_pos, align).ok_or(AllocError::InvalidParam)?;
        let end = start.checked_add(size).ok_or(AllocError::NoMemory)?;
        // Never overwrite the pages that were allocated from the top.
        if end > self.p_pos {
            return Err(AllocError::NoMemory);
        }

        self.b_pos = end;
        self.count += 1;
        NonNull::new(start as *mut u8).ok_or(AllocError::NoMemory)
    }

    fn dealloc(&mut self, _pos: NonNull<u8>, _layout: Layout) {
        // Freeing a single byte allocation is not supported. Only when every
        // allocation has been released the whole bytes-used area is reclaimed.
        if self.count > 0 {
            self.count -= 1;
            if self.count == 0 {
                self.b_pos = self.start;
            }
        }
    }

    fn total_bytes(&self) -> usize {
        self.end - self.start
    }

    fn used_bytes(&self) -> usize {
        self.b_pos - self.start
    }

    fn available_bytes(&self) -> usize {
        self.p_pos - self.b_pos
    }
}

impl<const SIZE: usize> PageAllocator for EarlyAllocator<SIZE> {
    const PAGE_SIZE: usize = SIZE;

    fn alloc_pages(&mut self, num_pages: usize, align_pow2: usize) -> AllocResult<usize> {
        let align = 1usize << align_pow2;
        let size = num_pages.checked_mul(SIZE).ok_or(AllocError::NoMemory)?;

        // Pages are allocated from the top of the region, downwards.
        let top = self.p_pos.checked_sub(size).ok_or(AllocError::NoMemory)?;
        let start = align_down(top, align);
        // Do not touch the bytes that are already allocated at the bottom.
        if start < self.b_pos {
            return Err(AllocError::NoMemory);
        }

        self.p_pos = start;
        Ok(start)
    }

    fn dealloc_pages(&mut self, _pos: usize, _num_pages: usize) {
        // Pages are never given back: the early allocator is only used during
        // the very first stage of the boot, where leaking is harmless.
    }

    fn total_pages(&self) -> usize {
        (self.end - self.start) / SIZE
    }

    fn used_pages(&self) -> usize {
        (self.end - self.p_pos) / SIZE
    }

    fn available_pages(&self) -> usize {
        (self.p_pos - self.b_pos) / SIZE
    }
}

/// Aligns `pos` up to the next multiple of `align` (a power of two).
fn align_up(pos: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    pos.checked_add(align - 1).map(|p| p & !(align - 1))
}

/// Aligns `pos` down to the previous multiple of `align` (a power of two).
fn align_down(pos: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    pos & !(align - 1)
}
