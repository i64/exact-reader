#![allow(unused)]
mod raw_vec;
mod unique;
mod utils;

use std::cmp::Ordering;
use std::ops::{Add, Index, IndexMut, Range, RangeBounds};
use std::slice;

use raw_vec::{RawVec, TryReserveError, TryReserveErrorKind};
use utils::slice_range;
pub struct VecDeque<T> {
    // `self[0]`, if it exists, is `buf[head]`.
    // `head < buf.capacity()`, unless `buf.capacity() == 0` when `head == 0`.
    head: usize,
    // the number of initialized elements, starting from the one at `head` and potentially wrapping around.
    // if `len == 0`, the exact value of `head` is unimportant.
    // if `T` is zero-Sized, then `self.len <= usize::MAX`, otherwise `self.len <= isize::MAX as usize`.
    len: usize,
    buf: RawVec<T>,
}
impl<T> Drop for VecDeque<T> {
    fn drop(&mut self) {
        struct Dropper<'a, T>(&'a mut [T]);
        impl<'a, T> Drop for Dropper<'a, T> {
            fn drop(&mut self) {
                unsafe {
                    std::ptr::drop_in_place(self.0);
                }
            }
        }
        let (front, back) = self.as_mut_slices();
        unsafe {
            let _back_dropper = Dropper(back);
            // use drop for [T]
            std::ptr::drop_in_place(front);
        }
        // RawVec handles deallocation
    }
}

impl<T> VecDeque<T> {
    #[inline]
    fn ptr(&self) -> *mut T {
        self.buf.ptr()
    }

    #[inline]
    unsafe fn buffer_read(&mut self, off: usize) -> T {
        unsafe { std::ptr::read(self.ptr().add(off)) }
    }
    #[inline]
    unsafe fn buffer_write(&mut self, off: usize, value: T) {
        unsafe {
            std::ptr::write(self.ptr().add(off), value);
        }
    }
    #[inline]
    unsafe fn buffer_range(&self, range: Range<usize>) -> *mut [T] {
        unsafe {
            std::ptr::slice_from_raw_parts_mut(self.ptr().add(range.start), range.end - range.start)
        }
    }
    #[inline]
    fn is_full(&self) -> bool {
        self.len == self.capacity()
    }

    #[inline]
    fn wrap_add(&self, idx: usize, addend: usize) -> usize {
        wrap_index(idx.wrapping_add(addend), self.capacity())
    }
    #[inline]
    fn to_physical_idx(&self, idx: usize) -> usize {
        self.wrap_add(self.head, idx)
    }
    #[inline]
    fn wrap_sub(&self, idx: usize, subtrahend: usize) -> usize {
        wrap_index(
            idx.wrapping_sub(subtrahend).wrapping_add(self.capacity()),
            self.capacity(),
        )
    }
    #[inline]
    unsafe fn copy(&mut self, src: usize, dst: usize, len: usize) {
        debug_assert!(
            dst + len <= self.capacity(),
            "cpy dst={} src={} len={} cap={}",
            dst,
            src,
            len,
            self.capacity()
        );
        debug_assert!(
            src + len <= self.capacity(),
            "cpy dst={} src={} len={} cap={}",
            dst,
            src,
            len,
            self.capacity()
        );
        unsafe {
            std::ptr::copy(self.ptr().add(src), self.ptr().add(dst), len);
        }
    }
    #[inline]
    unsafe fn copy_nonoverlapping(&mut self, src: usize, dst: usize, len: usize) {
        debug_assert!(
            dst + len <= self.capacity(),
            "cno dst={} src={} len={} cap={}",
            dst,
            src,
            len,
            self.capacity()
        );
        debug_assert!(
            src + len <= self.capacity(),
            "cno dst={} src={} len={} cap={}",
            dst,
            src,
            len,
            self.capacity()
        );
        unsafe {
            std::ptr::copy_nonoverlapping(self.ptr().add(src), self.ptr().add(dst), len);
        }
    }
    unsafe fn wrap_copy(&mut self, src: usize, dst: usize, len: usize) {
        debug_assert!(
            std::cmp::min(src.abs_diff(dst), self.capacity() - src.abs_diff(dst)) + len
                <= self.capacity(),
            "wrc dst={} src={} len={} cap={}",
            dst,
            src,
            len,
            self.capacity()
        );
        // If T is a ZST, don't do any copying.
        if src == dst || len == 0 {
            return;
        }
        let dst_after_src = self.wrap_sub(dst, src) < len;
        let src_pre_wrap_len = self.capacity() - src;
        let dst_pre_wrap_len = self.capacity() - dst;
        let src_wraps = src_pre_wrap_len < len;
        let dst_wraps = dst_pre_wrap_len < len;
        match (dst_after_src, src_wraps, dst_wraps) {
            (_, false, false) => {
                // src doesn't wrap, dst doesn't wrap
                //
                //        S . . .
                // 1 [_ _ A A B B C C _]
                // 2 [_ _ A A A A B B _]
                //            D . . .
                //
                unsafe {
                    self.copy(src, dst, len);
                }
            }
            (false, false, true) => {
                // dst before src, src doesn't wrap, dst wraps
                //
                //    S . . .
                // 1 [A A B B _ _ _ C C]
                // 2 [A A B B _ _ _ A A]
                // 3 [B B B B _ _ _ A A]
                //    . .           D .
                //
                unsafe {
                    self.copy(src, dst, dst_pre_wrap_len);
                    self.copy(src + dst_pre_wrap_len, 0, len - dst_pre_wrap_len);
                }
            }
            (true, false, true) => {
                // src before dst, src doesn't wrap, dst wraps
                //
                //              S . . .
                // 1 [C C _ _ _ A A B B]
                // 2 [B B _ _ _ A A B B]
                // 3 [B B _ _ _ A A A A]
                //    . .           D .
                //
                unsafe {
                    self.copy(src + dst_pre_wrap_len, 0, len - dst_pre_wrap_len);
                    self.copy(src, dst, dst_pre_wrap_len);
                }
            }
            (false, true, false) => {
                // dst before src, src wraps, dst doesn't wrap
                //
                //    . .           S .
                // 1 [C C _ _ _ A A B B]
                // 2 [C C _ _ _ B B B B]
                // 3 [C C _ _ _ B B C C]
                //              D . . .
                //
                unsafe {
                    self.copy(src, dst, src_pre_wrap_len);
                    self.copy(0, dst + src_pre_wrap_len, len - src_pre_wrap_len);
                }
            }
            (true, true, false) => {
                // src before dst, src wraps, dst doesn't wrap
                //
                //    . .           S .
                // 1 [A A B B _ _ _ C C]
                // 2 [A A A A _ _ _ C C]
                // 3 [C C A A _ _ _ C C]
                //    D . . .
                //
                unsafe {
                    self.copy(0, dst + src_pre_wrap_len, len - src_pre_wrap_len);
                    self.copy(src, dst, src_pre_wrap_len);
                }
            }
            (false, true, true) => {
                // dst before src, src wraps, dst wraps
                //
                //    . . .         S .
                // 1 [A B C D _ E F G H]
                // 2 [A B C D _ E G H H]
                // 3 [A B C D _ E G H A]
                // 4 [B C C D _ E G H A]
                //    . .         D . .
                //
                debug_assert!(dst_pre_wrap_len > src_pre_wrap_len);
                let delta = dst_pre_wrap_len - src_pre_wrap_len;
                unsafe {
                    self.copy(src, dst, src_pre_wrap_len);
                    self.copy(0, dst + src_pre_wrap_len, delta);
                    self.copy(delta, 0, len - dst_pre_wrap_len);
                }
            }
            (true, true, true) => {
                // src before dst, src wraps, dst wraps
                //
                //    . .         S . .
                // 1 [A B C D _ E F G H]
                // 2 [A A B D _ E F G H]
                // 3 [H A B D _ E F G H]
                // 4 [H A B D _ E F F G]
                //    . . .         D .
                //
                debug_assert!(src_pre_wrap_len > dst_pre_wrap_len);
                let delta = src_pre_wrap_len - dst_pre_wrap_len;
                unsafe {
                    self.copy(0, delta, len - src_pre_wrap_len);
                    self.copy(self.capacity() - delta, 0, delta);
                    self.copy(src, dst, dst_pre_wrap_len);
                }
            }
        }
    }
    #[inline]
    unsafe fn copy_slice(&mut self, dst: usize, src: &[T]) {
        debug_assert!(src.len() <= self.capacity());
        let head_room = self.capacity() - dst;
        if src.len() <= head_room {
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr(), self.ptr().add(dst), src.len());
            }
        } else {
            let (left, right) = src.split_at(head_room);
            unsafe {
                std::ptr::copy_nonoverlapping(left.as_ptr(), self.ptr().add(dst), left.len());
                std::ptr::copy_nonoverlapping(right.as_ptr(), self.ptr(), right.len());
            }
        }
    }
    #[inline]
    unsafe fn write_iter(
        &mut self,
        dst: usize,
        iter: impl Iterator<Item = T>,
        written: &mut usize,
    ) {
        iter.enumerate().for_each(|(i, element)| unsafe {
            self.buffer_write(dst + i, element);
            *written += 1;
        });
    }
    #[inline]
    unsafe fn handle_capacity_increase(&mut self, old_capacity: usize) {
        let new_capacity = self.capacity();
        debug_assert!(new_capacity >= old_capacity);
        // Move the shortest contiguous section of the ring buffer
        //
        // H := head
        // L := last element (`self.to_physical_idx(self.len - 1)`)
        //
        //    H           L
        //   [o o o o o o o . ]
        //    H           L
        // A [o o o o o o o . . . . . . . . . ]
        //        L H
        //   [o o o o o o o o ]
        //          H           L
        // B [. . . o o o o o o o . . . . . . ]
        //              L H
        //   [o o o o o o o o ]
        //            L                   H
        // C [o o o o o . . . . . . . . . o o ]
        // can't use is_contiguous() because the capacity is already updated.
        if self.head <= old_capacity - self.len {
            // A
            // Nop
        } else {
            let head_len = old_capacity - self.head;
            let tail_len = self.len - head_len;
            if head_len > tail_len && new_capacity - old_capacity >= tail_len {
                // B
                unsafe {
                    self.copy_nonoverlapping(0, old_capacity, tail_len);
                }
            } else {
                // C
                let new_head = new_capacity - head_len;
                unsafe {
                    // can't use copy_nonoverlapping here, because if e.g. head_len = 2
                    // and new_capacity = old_capacity + 1, then the heads overlap.
                    self.copy(self.head, new_head, head_len);
                }
                self.head = new_head;
            }
        }
        debug_assert!(self.head < self.capacity() || self.capacity() == 0);
    }

    pub fn clear_front(&mut self, count: usize) {
        if self.len > count {
            self.head = self.to_physical_idx(count);
            self.len -= count
        }
    }

    pub fn clear_back(&mut self, count: usize) {
        if self.len > count {
            self.len -= count
        }
    }

    pub fn extend_back(&mut self, extend_from: &[T]) {
        let len = extend_from.len();

        if len == 0 {
            return;
        }

        self.reserve(extend_from.len());
        let tail = self.to_physical_idx(self.len);

        unsafe { std::ptr::copy_nonoverlapping(extend_from.as_ptr(), self.ptr().add(tail), len) }
        self.len += len;
    }

    pub fn extend_front(&mut self, extend_from: &[T]) {
        let len = extend_from.len();

        if len == 0 {
            return;
        }

        self.reserve(len);
        self.head = self.wrap_sub(self.head, len);

        unsafe {
            std::ptr::copy_nonoverlapping(extend_from.as_ptr(), self.ptr().add(self.head), len)
        }

        self.len += len;
    }
}
impl<T> VecDeque<T> {
    #[inline]
    #[must_use]
    pub const fn new() -> VecDeque<T> {
        // FIXME: This should just be `VecDeque::new_in(Global)` once that hits stable.
        VecDeque {
            head: 0,
            len: 0,
            buf: RawVec::NEW,
        }
    }
    #[inline]
    #[must_use]
    pub fn with_capacity(capacity: usize) -> VecDeque<T> {
        Self::with_capacity_in(capacity)
    }
}
impl<T> VecDeque<T> {
    #[inline]
    pub const fn new_in() -> VecDeque<T> {
        VecDeque {
            head: 0,
            len: 0,
            buf: RawVec::new_in(),
        }
    }
    pub fn with_capacity_in(capacity: usize) -> VecDeque<T> {
        VecDeque {
            head: 0,
            len: 0,
            buf: RawVec::with_capacity(capacity),
        }
    }
    #[inline]
    pub(crate) unsafe fn from_contiguous_raw_parts_in(
        ptr: *mut T,
        initialized: Range<usize>,
        capacity: usize,
    ) -> Self {
        debug_assert!(initialized.start <= initialized.end);
        debug_assert!(initialized.end <= capacity);
        // SAFETY: Our safety precondition guarantees the range length won't wrap,
        // and that the allocation is valid for use in `RawVec`.
        unsafe {
            VecDeque {
                head: initialized.start,
                len: initialized.end - initialized.start,
                buf: RawVec::from_raw_parts(ptr, capacity),
            }
        }
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        if index < self.len {
            let idx = self.to_physical_idx(index);
            unsafe { Some(&*self.ptr().add(idx)) }
        } else {
            None
        }
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index < self.len {
            let idx = self.to_physical_idx(index);
            unsafe { Some(&mut *self.ptr().add(idx)) }
        } else {
            None
        }
    }
    pub fn swap(&mut self, i: usize, j: usize) {
        assert!(i < self.len());
        assert!(j < self.len());
        let ri = self.to_physical_idx(i);
        let rj = self.to_physical_idx(j);
        unsafe { std::ptr::swap(self.ptr().add(ri), self.ptr().add(rj)) }
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }
    pub fn reserve_exact(&mut self, additional: usize) {
        let new_cap = self.len.checked_add(additional).expect("capacity overflow");
        let old_cap = self.capacity();
        if new_cap > old_cap {
            self.buf.reserve_exact(self.len, additional);
            unsafe {
                self.handle_capacity_increase(old_cap);
            }
        }
    }
    pub fn reserve(&mut self, additional: usize) {
        let new_cap = self.len.checked_add(additional).expect("capacity overflow");
        let old_cap = self.capacity();
        if new_cap > old_cap {
            // we don't need to reserve_exact(), as the size doesn't have
            // to be a power of 2.
            self.buf.reserve(self.len, additional);
            unsafe {
                self.handle_capacity_increase(old_cap);
            }
        }
    }
    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), TryReserveError> {
        let new_cap = self
            .len
            .checked_add(additional)
            .ok_or(TryReserveErrorKind::CapacityOverflow)?;
        let old_cap = self.capacity();
        if new_cap > old_cap {
            self.buf.try_reserve_exact(self.len, additional)?;
            unsafe {
                self.handle_capacity_increase(old_cap);
            }
        }
        Ok(())
    }
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), TryReserveError> {
        let new_cap = self
            .len
            .checked_add(additional)
            .ok_or(TryReserveErrorKind::CapacityOverflow)?;
        let old_cap = self.capacity();
        if new_cap > old_cap {
            self.buf.try_reserve(self.len, additional)?;
            unsafe {
                self.handle_capacity_increase(old_cap);
            }
        }
        Ok(())
    }
    pub fn shrink_to_fit(&mut self) {
        self.shrink_to(0);
    }
    pub fn shrink_to(&mut self, min_capacity: usize) {
        let target_cap = min_capacity.max(self.len);
        // never shrink ZSTs
        if self.capacity() <= target_cap {
            return;
        }
        // There are three cases of interest:
        //   All elements are out of desired bounds
        //   Elements are contiguous, and tail is out of desired bounds
        //   Elements are discontiguous
        //
        // At all other times, element positions are unaffected.
        // `head` and `len` are at most `isize::MAX` and `target_cap < self.capacity()`, so nothing can
        // overflow.
        let tail_outside = (target_cap + 1..=self.capacity()).contains(&(self.head + self.len));
        if self.len == 0 {
            self.head = 0;
        } else if self.head >= target_cap && tail_outside {
            // Head and tail are both out of bounds, so copy all of them to the front.
            //
            //  H := head
            //  L := last element
            //                    H           L
            //   [. . . . . . . . o o o o o o o . ]
            //    H           L
            //   [o o o o o o o . ]
            unsafe {
                // nonoverlapping because `self.head >= target_cap >= self.len`.
                self.copy_nonoverlapping(self.head, 0, self.len);
            }
            self.head = 0;
        } else if self.head < target_cap && tail_outside {
            // Head is in bounds, tail is out of bounds.
            // Copy the overflowing part to the beginning of the
            // buffer. This won't overlap because `target_cap >= self.len`.
            //
            //  H := head
            //  L := last element
            //          H           L
            //   [. . . o o o o o o o . . . . . . ]
            //      L   H
            //   [o o . o o o o o ]
            let len = self.head + self.len - target_cap;
            unsafe {
                self.copy_nonoverlapping(target_cap, 0, len);
            }
        } else if !self.is_contiguous() {
            // The head slice is at least partially out of bounds, tail is in bounds.
            // Copy the head backwards so it lines up with the target capacity.
            // This won't overlap because `target_cap >= self.len`.
            //
            //  H := head
            //  L := last element
            //            L                   H
            //   [o o o o o . . . . . . . . . o o ]
            //            L   H
            //   [o o o o o . o o ]
            let head_len = self.capacity() - self.head;
            let new_head = target_cap - head_len;
            unsafe {
                // can't use `copy_nonoverlapping()` here because the new and old
                // regions for the head might overlap.
                self.copy(self.head, new_head, head_len);
            }
            self.head = new_head;
        }
        self.buf.shrink_to_fit(target_cap);
        debug_assert!(self.head < self.capacity() || self.capacity() == 0);
        debug_assert!(self.len <= self.capacity());
    }
    pub fn truncate(&mut self, len: usize) {
        struct Dropper<'a, T>(&'a mut [T]);
        impl<'a, T> Drop for Dropper<'a, T> {
            fn drop(&mut self) {
                unsafe {
                    std::ptr::drop_in_place(self.0);
                }
            }
        }
        // Safe because:
        //
        // * Any slice passed to `drop_in_place` is valid; the second case has
        //   `len <= front.len()` and returning on `len > self.len()` ensures
        //   `begin <= back.len()` in the first case
        // * The head of the VecDeque is moved before calling `drop_in_place`,
        //   so no value is dropped twice if `drop_in_place` panics
        unsafe {
            if len >= self.len {
                return;
            }
            let (front, back) = self.as_mut_slices();
            if len > front.len() {
                let begin = len - front.len();
                let drop_back = back.get_unchecked_mut(begin..) as *mut _;
                self.len = len;
                std::ptr::drop_in_place(drop_back);
            } else {
                let drop_back = back as *mut _;
                let drop_front = front.get_unchecked_mut(len..) as *mut _;
                self.len = len;
                // Make sure the second half is dropped even when a destructor
                // in the first one panics.
                let _back_dropper = Dropper(&mut *drop_back);
                std::ptr::drop_in_place(drop_front);
            }
        }
    }

    #[inline]
    pub fn as_slices(&self) -> (&[T], &[T]) {
        let (a_range, b_range) = self.slice_ranges(.., self.len);
        // SAFETY: `slice_ranges` always returns valid ranges into
        // the physical buffer.
        unsafe { (&*self.buffer_range(a_range), &*self.buffer_range(b_range)) }
    }
    #[inline]
    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        let (a_range, b_range) = self.slice_ranges(.., self.len);
        // SAFETY: `slice_ranges` always returns valid ranges into
        // the physical buffer.
        unsafe {
            (
                &mut *self.buffer_range(a_range),
                &mut *self.buffer_range(b_range),
            )
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn slice_ranges<R>(&self, range: R, len: usize) -> (Range<usize>, Range<usize>)
    where
        R: RangeBounds<usize>,
    {
        let Range { start, end } = slice_range(range, ..len);
        let len = end - start;
        if len == 0 {
            (0..0, 0..0)
        } else {
            // `slice_range` guarantees that `start <= end <= len`.
            // because `len != 0`, we know that `start < end`, so `start < len`
            // and the indexing is valid.
            let wrapped_start = self.to_physical_idx(start);
            // this subtraction can never overflow because `wrapped_start` is
            // at most `self.capacity()` (and if `self.capacity != 0`, then `wrapped_start` is strictly less
            // than `self.capacity`).
            let head_len = self.capacity() - wrapped_start;
            if head_len >= len {
                // we know that `len + wrapped_start <= self.capacity <= usize::MAX`, so this addition can't overflow
                (wrapped_start..wrapped_start + len, 0..0)
            } else {
                // can't overflow because of the if condition
                let tail_len = len - head_len;
                (wrapped_start..self.capacity(), 0..tail_len)
            }
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.truncate(0);
        // Not strictly necessary, but leaves things in a more consistent/predictable state.
        self.head = 0;
    }
    pub fn contains(&self, x: &T) -> bool
    where
        T: PartialEq<T>,
    {
        let (a, b) = self.as_slices();
        a.contains(x) || b.contains(x)
    }
    pub fn front(&self) -> Option<&T> {
        self.get(0)
    }
    pub fn front_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
    }
    pub fn back(&self) -> Option<&T> {
        self.get(self.len.wrapping_sub(1))
    }
    pub fn back_mut(&mut self) -> Option<&mut T> {
        self.get_mut(self.len.wrapping_sub(1))
    }
    pub fn pop_front(&mut self) -> Option<T> {
        if self.is_empty() {
            None
        } else {
            let old_head = self.head;
            self.head = self.to_physical_idx(1);
            self.len -= 1;
            Some(unsafe { self.buffer_read(old_head) })
        }
    }
    pub fn pop_back(&mut self) -> Option<T> {
        if self.is_empty() {
            None
        } else {
            self.len -= 1;
            Some(unsafe { self.buffer_read(self.to_physical_idx(self.len)) })
        }
    }
    pub fn push_front(&mut self, value: T) {
        if self.is_full() {
            self.grow();
        }
        self.head = self.wrap_sub(self.head, 1);
        self.len += 1;
        unsafe {
            self.buffer_write(self.head, value);
        }
    }
    pub fn push_back(&mut self, value: T) {
        if self.is_full() {
            self.grow();
        }
        unsafe { self.buffer_write(self.to_physical_idx(self.len), value) }
        self.len += 1;
    }
    #[inline]
    fn is_contiguous(&self) -> bool {
        // Do the calculation like this to avoid overflowing if len + head > usize::MAX
        self.head <= self.capacity() - self.len
    }
    pub fn swap_remove_front(&mut self, index: usize) -> Option<T> {
        let length = self.len;
        if index < length && index != 0 {
            self.swap(index, 0);
        } else if index >= length {
            return None;
        }
        self.pop_front()
    }
    pub fn swap_remove_back(&mut self, index: usize) -> Option<T> {
        let length = self.len;
        if length > 0 && index < length - 1 {
            self.swap(index, length - 1);
        } else if index >= length {
            return None;
        }
        self.pop_back()
    }
    pub fn insert(&mut self, index: usize, value: T) {
        assert!(index <= self.len(), "index out of bounds");
        if self.is_full() {
            self.grow();
        }
        let k = self.len - index;
        if k < index {
            // `index + 1` can't overflow, because if index was usize::MAX, then either the
            // assert would've failed, or the deque would've tried to grow past usize::MAX
            // and panicked.
            unsafe {
                // see `remove()` for explanation why this wrap_copy() call is safe.
                self.wrap_copy(
                    self.to_physical_idx(index),
                    self.to_physical_idx(index + 1),
                    k,
                );
                self.buffer_write(self.to_physical_idx(index), value);
                self.len += 1;
            }
        } else {
            let old_head = self.head;
            self.head = self.wrap_sub(self.head, 1);
            unsafe {
                self.wrap_copy(old_head, self.head, index);
                self.buffer_write(self.to_physical_idx(index), value);
                self.len += 1;
            }
        }
    }
    pub fn remove(&mut self, index: usize) -> Option<T> {
        if self.len <= index {
            return None;
        }
        let wrapped_idx = self.to_physical_idx(index);
        let elem = unsafe { Some(self.buffer_read(wrapped_idx)) };
        let k = self.len - index - 1;
        // safety: due to the nature of the if-condition, whichever wrap_copy gets called,
        // its length argument will be at most `self.len / 2`, so there can't be more than
        // one overlapping area.
        if k < index {
            unsafe { self.wrap_copy(self.wrap_add(wrapped_idx, 1), wrapped_idx, k) };
            self.len -= 1;
        } else {
            let old_head = self.head;
            self.head = self.to_physical_idx(1);
            unsafe { self.wrap_copy(old_head, self.head, index) };
            self.len -= 1;
        }
        elem
    }
    #[inline]
    #[must_use = "use `.truncate()` if you don't need the other half"]
    pub fn split_off(&mut self, at: usize) -> Self {
        let len = self.len;
        assert!(at <= len, "`at` out of bounds");
        let other_len = len - at;
        let mut other = VecDeque::with_capacity_in(other_len);
        unsafe {
            let (first_half, second_half) = self.as_slices();
            let first_len = first_half.len();
            let second_len = second_half.len();
            if at < first_len {
                // `at` lies in the first half.
                let amount_in_first = first_len - at;
                std::ptr::copy_nonoverlapping(
                    first_half.as_ptr().add(at),
                    other.ptr(),
                    amount_in_first,
                );
                // just take all of the second half.
                std::ptr::copy_nonoverlapping(
                    second_half.as_ptr(),
                    other.ptr().add(amount_in_first),
                    second_len,
                );
            } else {
                // `at` lies in the second half, need to factor in the elements we skipped
                // in the first half.
                let offset = at - first_len;
                let amount_in_second = second_len - offset;
                std::ptr::copy_nonoverlapping(
                    second_half.as_ptr().add(offset),
                    other.ptr(),
                    amount_in_second,
                );
            }
        }
        // Cleanup where the ends of the buffers are
        self.len = at;
        other.len = other_len;
        other
    }
    #[inline]
    pub fn append(&mut self, other: &mut Self) {
        self.reserve(other.len);
        unsafe {
            let (left, right) = other.as_slices();
            self.copy_slice(self.to_physical_idx(self.len), left);
            // no overflow, because self.capacity() >= old_cap + left.len() >= self.len + left.len()
            self.copy_slice(self.to_physical_idx(self.len + left.len()), right);
        }
        // SAFETY: Update pointers after copying to avoid leaving doppelganger
        // in case of panics.
        self.len += other.len;
        // Now that we own its values, forget everything in `other`.
        other.len = 0;
        other.head = 0;
    }
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.retain_mut(|elem| f(elem));
    }
    pub fn retain_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        let len = self.len;
        let mut idx = 0;
        let mut cur = 0;
        // Stage 1: All values are retained.
        while cur < len {
            if !f(&mut self[cur]) {
                cur += 1;
                break;
            }
            cur += 1;
            idx += 1;
        }
        // Stage 2: Swap retained value into current idx.
        while cur < len {
            if !f(&mut self[cur]) {
                cur += 1;
                continue;
            }
            self.swap(idx, cur);
            cur += 1;
            idx += 1;
        }
        // Stage 3: Truncate all values after idx.
        if cur != idx {
            self.truncate(idx);
        }
    }
    // Double the buffer size. This method is inline(never), so we expect it to only
    // be called in cold paths.
    // This may panic or abort
    #[inline(never)]
    fn grow(&mut self) {
        println!("=== growing");

        // Extend or possibly remove this assertion when valid use-cases for growing the
        // buffer without it being full emerge
        debug_assert!(self.is_full());
        let old_cap = self.capacity();
        self.buf.reserve_for_push(old_cap);
        unsafe {
            self.handle_capacity_increase(old_cap);
        }
        debug_assert!(!self.is_full());
    }

    pub fn make_contiguous(&mut self) -> &mut [T] {
        if self.is_contiguous() {
            unsafe { return slice::from_raw_parts_mut(self.ptr().add(self.head), self.len) }
        }
        let &mut Self { head, len, .. } = self;
        let ptr = self.ptr();
        let cap = self.capacity();
        let free = cap - len;
        let head_len = cap - head;
        let tail = len - head_len;
        let tail_len = tail;
        if free >= head_len {
            // there is enough free space to copy the head in one go,
            // this means that we first shift the tail backwards, and then
            // copy the head to the correct position.
            //
            // from: DEFGH....ABC
            // to:   ABCDEFGH....
            unsafe {
                self.copy(0, head_len, tail_len);
                // ...DEFGH.ABC
                self.copy_nonoverlapping(head, 0, head_len);
                // ABCDEFGH....
            }
            self.head = 0;
        } else if free >= tail_len {
            // there is enough free space to copy the tail in one go,
            // this means that we first shift the head forwards, and then
            // copy the tail to the correct position.
            //
            // from: FGH....ABCDE
            // to:   ...ABCDEFGH.
            unsafe {
                self.copy(head, tail, head_len);
                // FGHABCDE....
                self.copy_nonoverlapping(0, tail + head_len, tail_len);
                // ...ABCDEFGH.
            }
            self.head = tail;
        } else {
            // `free` is smaller than both `head_len` and `tail_len`.
            // the general algorithm for this first moves the slices
            // right next to each other and then uses `slice::rotate`
            // to rotate them into place:
            //
            // initially:   HIJK..ABCDEFG
            // step 1:      ..HIJKABCDEFG
            // step 2:      ..ABCDEFGHIJK
            //
            // or:
            //
            // initially:   FGHIJK..ABCDE
            // step 1:      FGHIJKABCDE..
            // step 2:      ABCDEFGHIJK..
            // pick the shorter of the 2 slices to reduce the amount
            // of memory that needs to be moved around.
            if head_len > tail_len {
                // tail is shorter, so:
                //  1. copy tail forwards
                //  2. rotate used part of the buffer
                //  3. update head to point to the new beginning (which is just `free`)
                unsafe {
                    // if there is no free space in the buffer, then the slices are already
                    // right next to each other and we don't need to move any memory.
                    if free != 0 {
                        // because we only move the tail forward as much as there's free space
                        // behind it, we don't overwrite any elements of the head slice, and
                        // the slices end up right next to each other.
                        self.copy(0, free, tail_len);
                    }
                    // We just copied the tail right next to the head slice,
                    // so all of the elements in the range are initialized
                    let slice = &mut *self.buffer_range(free..self.capacity());
                    // because the deque wasn't contiguous, we know that `tail_len < self.len == slice.len()`,
                    // so this will never panic.
                    slice.rotate_left(tail_len);
                    // the used part of the buffer now is `free..self.capacity()`, so set
                    // `head` to the beginning of that range.
                    self.head = free;
                }
            } else {
                // head is shorter so:
                //  1. copy head backwards
                //  2. rotate used part of the buffer
                //  3. update head to point to the new beginning (which is the beginning of the buffer)
                unsafe {
                    // if there is no free space in the buffer, then the slices are already
                    // right next to each other and we don't need to move any memory.
                    if free != 0 {
                        // copy the head slice to lie right behind the tail slice.
                        self.copy(self.head, tail_len, head_len);
                    }
                    // because we copied the head slice so that both slices lie right
                    // next to each other, all the elements in the range are initialized.
                    let slice = &mut *self.buffer_range(0..self.len);
                    // because the deque wasn't contiguous, we know that `head_len < self.len == slice.len()`
                    // so this will never panic.
                    slice.rotate_right(head_len);
                    // the used part of the buffer now is `0..self.len`, so set
                    // `head` to the beginning of that range.
                    self.head = 0;
                }
            }
        }
        unsafe { slice::from_raw_parts_mut(ptr.add(self.head), self.len) }
    }
    pub fn rotate_left(&mut self, n: usize) {
        assert!(n <= self.len());
        let k = self.len - n;
        if n <= k {
            unsafe { self.rotate_left_inner(n) }
        } else {
            unsafe { self.rotate_right_inner(k) }
        }
    }
    pub fn rotate_right(&mut self, n: usize) {
        assert!(n <= self.len());
        let k = self.len - n;
        if n <= k {
            unsafe { self.rotate_right_inner(n) }
        } else {
            unsafe { self.rotate_left_inner(k) }
        }
    }
    // SAFETY: the following two methods require that the rotation amount
    // be less than half the length of the deque.
    //
    // `wrap_copy` requires that `min(x, capacity() - x) + copy_len <= capacity()`,
    // but then `min` is never more than half the capacity, regardless of x,
    // so it's sound to call here because we're calling with something
    // less than half the length, which is never above half the capacity.
    unsafe fn rotate_left_inner(&mut self, mid: usize) {
        debug_assert!(mid * 2 <= self.len());
        unsafe {
            self.wrap_copy(self.head, self.to_physical_idx(self.len), mid);
        }
        self.head = self.to_physical_idx(mid);
    }
    unsafe fn rotate_right_inner(&mut self, k: usize) {
        debug_assert!(k * 2 <= self.len());
        self.head = self.wrap_sub(self.head, k);
        unsafe {
            self.wrap_copy(self.to_physical_idx(self.len), self.head, k);
        }
    }
    #[inline]
    pub fn binary_search(&self, x: &T) -> Result<usize, usize>
    where
        T: Ord,
    {
        self.binary_search_by(|e| e.cmp(x))
    }
    pub fn binary_search_by<'a, F>(&'a self, mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&'a T) -> Ordering,
    {
        let (front, back) = self.as_slices();
        let cmp_back = back.first().map(&mut f);
        if let Some(Ordering::Equal) = cmp_back {
            Ok(front.len())
        } else if let Some(Ordering::Less) = cmp_back {
            back.binary_search_by(f)
                .map(|idx| idx + front.len())
                .map_err(|idx| idx + front.len())
        } else {
            front.binary_search_by(f)
        }
    }
    #[inline]
    pub fn binary_search_by_key<'a, B, F>(&'a self, b: &B, mut f: F) -> Result<usize, usize>
    where
        F: FnMut(&'a T) -> B,
        B: Ord,
    {
        self.binary_search_by(|k| f(k).cmp(b))
    }
    pub fn partition_point<P>(&self, mut pred: P) -> usize
    where
        P: FnMut(&T) -> bool,
    {
        let (front, back) = self.as_slices();
        if let Some(true) = back.first().map(&mut pred) {
            back.partition_point(pred) + front.len()
        } else {
            front.partition_point(pred)
        }
    }
}
#[inline]
fn wrap_index(logical_index: usize, capacity: usize) -> usize {
    debug_assert!(
        (logical_index == 0 && capacity == 0)
            || logical_index < capacity
            || (logical_index - capacity) < capacity
    );
    if logical_index >= capacity {
        logical_index - capacity
    } else {
        logical_index
    }
}

impl<T> Index<usize> for VecDeque<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("Out of bounds access")
    }
}

impl<T> IndexMut<usize> for VecDeque<T> {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index).expect("Out of bounds access")
    }
}
