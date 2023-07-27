use core::alloc::LayoutError;
use core::cmp;
use core::ops::Drop;
use core::ptr::{self, NonNull};
use std::alloc::{handle_alloc_error, Layout};

pub struct TryReserveError {
    pub kind: TryReserveErrorKind,
}

impl TryReserveError {
    pub fn kind(&self) -> TryReserveErrorKind {
        self.kind.clone()
    }
}

impl From<TryReserveErrorKind> for TryReserveError {
    fn from(kind: TryReserveErrorKind) -> Self {
        TryReserveError { kind }
    }
}
#[derive(Clone)]
pub enum TryReserveErrorKind {
    CapacityOverflow,
    AllocError { layout: Layout, non_exhaustive: () },
}
#[cfg(not(no_global_oom_handling))]
#[allow(dead_code)]
enum AllocInit {
    Uninitialized,
    Zeroed,
}
#[allow(missing_debug_implementations)]
pub struct RawVec<T> {
    ptr: NonNull<T>,
    cap: usize,
}
#[allow(dead_code)]
impl<T> RawVec<T> {
    #[allow(dead_code)]
    pub const NEW: Self = Self::new();
    #[must_use]
    pub const fn new() -> Self {
        Self::new_in()
    }
    pub fn with_capacity(capacity: usize) -> Self {
        Self::allocate_in(capacity, AllocInit::Uninitialized)
    }
    pub fn with_capacity_zeroed(capacity: usize) -> Self {
        Self::allocate_in(capacity, AllocInit::Zeroed)
    }
}
#[allow(dead_code)]
impl<T> RawVec<T> {
    pub(crate) const MIN_NON_ZERO_CAP: usize = 8;
    pub const fn new_in() -> Self {
        Self {
            ptr: NonNull::dangling(),
            cap: 0,
        }
    }
    #[cfg(not(no_global_oom_handling))]
    fn allocate_in(capacity: usize, init: AllocInit) -> Self {
        let layout = match Layout::array::<u8>(capacity) {
            Ok(layout) => layout,
            Err(_) => capacity_overflow(),
        };
        match alloc_guard(layout.size()) {
            Ok(_) => {}
            Err(_) => capacity_overflow(),
        }
        let result = match init {
            AllocInit::Uninitialized => unsafe { std::alloc::alloc(layout) },
            AllocInit::Zeroed => unsafe { std::alloc::alloc_zeroed(layout) },
        };
        Self {
            ptr: unsafe { NonNull::new_unchecked(result.cast()) },
            cap: capacity,
        }
    }
    #[inline]
    pub unsafe fn from_raw_parts(ptr: *mut T, capacity: usize) -> Self {
        Self {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
            cap: capacity,
        }
    }
    #[inline]
    pub fn ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.cap
    }
    fn current_memory(&self) -> Option<(NonNull<u8>, Layout)> {
        if self.cap == 0 {
            None
        } else {
            {
                assert!(std::mem::size_of::<T>() % std::mem::align_of::<T>() == 0)
            };
            unsafe {
                let align = std::mem::align_of::<T>();
                let size = std::mem::size_of::<T>() * self.cap;
                let layout = Layout::from_size_align_unchecked(size, align);
                Some((self.ptr.cast(), layout))
            }
        }
    }

    #[cfg(not(no_global_oom_handling))]
    #[inline]
    pub fn reserve(&mut self, len: usize, additional: usize) {
        #[cold]
        fn do_reserve_and_handle<T>(slf: &mut RawVec<T>, len: usize, additional: usize) {
            handle_reserve(slf.grow_amortized(len, additional));
        }
        if self.needs_to_grow(len, additional) {
            do_reserve_and_handle(self, len, additional);
        }
    }
    #[cfg(not(no_global_oom_handling))]
    #[inline(never)]
    pub fn reserve_for_push(&mut self, len: usize) {
        handle_reserve(self.grow_amortized(len, 1));
    }
    pub fn try_reserve(&mut self, len: usize, additional: usize) -> Result<(), TryReserveError> {
        if self.needs_to_grow(len, additional) {
            self.grow_amortized(len, additional)
        } else {
            Ok(())
        }
    }
    #[cfg(not(no_global_oom_handling))]
    pub fn reserve_exact(&mut self, len: usize, additional: usize) {
        handle_reserve(self.try_reserve_exact(len, additional));
    }
    pub fn try_reserve_exact(
        &mut self,
        len: usize,
        additional: usize,
    ) -> Result<(), TryReserveError> {
        if self.needs_to_grow(len, additional) {
            self.grow_exact(len, additional)
        } else {
            Ok(())
        }
    }
    #[cfg(not(no_global_oom_handling))]
    pub fn shrink_to_fit(&mut self, cap: usize) {
        handle_reserve(self.shrink(cap));
    }
}

impl<T> RawVec<T> {
    fn needs_to_grow(&self, len: usize, additional: usize) -> bool {
        additional > self.capacity().wrapping_sub(len)
    }
    fn set_ptr_and_cap(&mut self, ptr: NonNull<[u8]>, cap: usize) {
        self.ptr = unsafe { NonNull::new_unchecked(ptr.as_ptr()).cast() };
        self.cap = cap;
    }
    fn grow_amortized(&mut self, len: usize, additional: usize) -> Result<(), TryReserveError> {
        debug_assert!(additional > 0);
        let required_cap = match len.checked_add(additional) {
            None => {
                return Err(TryReserveError {
                    kind: TryReserveErrorKind::CapacityOverflow,
                })
            }
            Some(c) => c,
        };
        let cap = cmp::max(self.cap * 2, required_cap);
        let cap = cmp::max(Self::MIN_NON_ZERO_CAP, cap);
        let new_layout = Layout::array::<T>(cap);
        let ptr = finish_grow(new_layout, self.current_memory())?;
        self.set_ptr_and_cap(ptr, cap);
        Ok(())
    }
    fn grow_exact(&mut self, len: usize, additional: usize) -> Result<(), TryReserveError> {
        let cap = match len.checked_add(additional) {
            None => {
                return Err(TryReserveError {
                    kind: TryReserveErrorKind::CapacityOverflow,
                })
            }
            Some(cap) => cap,
        };
        let new_layout = Layout::array::<T>(cap);
        let ptr = finish_grow(new_layout, self.current_memory())?;
        self.set_ptr_and_cap(ptr, cap);
        Ok(())
    }
    #[cfg(not(no_global_oom_handling))]
    fn shrink(&mut self, cap: usize) -> Result<(), TryReserveError> {
        assert!(
            cap <= self.capacity(),
            "Tried to shrink to a larger capacity"
        );

        let (ptr, layout) = if let Some(mem) = self.current_memory() {
            mem
        } else {
            return Ok(());
        };
        // See current_memory() why this assert is here
        {
            assert!(std::mem::size_of::<T>() % std::mem::align_of::<T>() == 0)
        };

        // If shrinking to 0, deallocate the buffer. We don't reach this point
        // for the T::IS_ZST case since current_memory() will have returned
        // None.
        if cap == 0 {
            unsafe { std::alloc::dealloc(ptr.as_ptr(), layout) };
            self.ptr = NonNull::dangling();
            self.cap = 0;
        } else {
            let ptr = unsafe {
                // `Layout::array` cannot overflow here because it would have
                // overflowed earlier when capacity was larger.
                let new_size = std::mem::size_of::<T>() * cap;
                let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
                unsafe { alloc_shrink(ptr, layout, new_layout) }
            };
            self.set_ptr_and_cap(ptr, cap);
        }
        Ok(())
    }
}

unsafe fn alloc_shrink(ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> NonNull<[u8]> {
    debug_assert!(
        new_layout.size() <= old_layout.size(),
        "`new_layout.size()` must be smaller than or equal to `old_layout.size()`"
    );

    let new_ptr = std::alloc::alloc(new_layout);

    // SAFETY: because `new_layout.size()` must be lower than or equal to
    // `old_layout.size()`, both the old and new memory allocation are valid for reads and
    // writes for `new_layout.size()` bytes. Also, because the old allocation wasn't yet
    // deallocated, it cannot overlap `new_ptr`. Thus, the call to `copy_nonoverlapping` is
    // safe. The safety contract for `dealloc` must be upheld by the caller.
    unsafe {
        std::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr, new_layout.size());
        std::alloc::dealloc(ptr.as_ptr(), old_layout);
    }

    NonNull::slice_from_raw_parts(
        unsafe { NonNull::new_unchecked(new_ptr) },
        new_layout.size(),
    )
}

#[inline(never)]
fn finish_grow(
    new_layout: Result<Layout, LayoutError>,
    current_memory: Option<(NonNull<u8>, Layout)>,
) -> Result<NonNull<[u8]>, TryReserveError> {
    // Check for the error here to minimize the size of `RawVec::grow_*`.
    let new_layout = new_layout.map_err(|_| TryReserveErrorKind::CapacityOverflow)?;

    alloc_guard(new_layout.size())?;
    let memory = if let Some((ptr, old_layout)) = current_memory {
        debug_assert_eq!(old_layout.align(), new_layout.align());
        unsafe {
            // The allocator checks for alignment equality
            assume(old_layout.align() == new_layout.align());
            global_grow(ptr, old_layout, new_layout)
        }
    } else {
        let new_ptr = unsafe { std::alloc::alloc(new_layout) };

        NonNull::slice_from_raw_parts(
            unsafe { NonNull::new_unchecked(new_ptr) },
            new_layout.size(),
        )
    };

    Ok(memory)
}

unsafe fn global_grow(ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> NonNull<[u8]> {
    debug_assert!(
        new_layout.size() >= old_layout.size(),
        "`new_layout.size()` must be greater than or equal to `old_layout.size()`"
    );

    let new_ptr = std::alloc::alloc(new_layout);

    unsafe {
        ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr, old_layout.size());
        std::alloc::dealloc(ptr.as_ptr(), old_layout);
    }

    NonNull::slice_from_raw_parts(NonNull::new_unchecked(new_ptr), new_layout.size())
}

impl<T> Drop for RawVec<T> {
    /// Frees the memory owned by the `RawVec` *without* trying to drop its contents.
    #[inline(always)]
    fn drop(&mut self) {
        if let Some((ptr, layout)) = self.current_memory() {
            unsafe { std::alloc::dealloc(ptr.as_ptr(), layout) }
        }
    }
}
#[inline]
fn handle_reserve(result: Result<(), TryReserveError>) {
    match result.map_err(|e| e.kind()) {
        Err(TryReserveErrorKind::CapacityOverflow) => capacity_overflow(),
        Err(TryReserveErrorKind::AllocError { layout, .. }) => handle_alloc_error(layout),
        Ok(()) => { /* yay */ }
    }
}
#[inline]
fn alloc_guard(alloc_size: usize) -> Result<(), TryReserveError> {
    if usize::BITS < 64 && alloc_size > isize::MAX as usize {
        Err(TryReserveErrorKind::CapacityOverflow.into())
    } else {
        Ok(())
    }
}
#[cfg(not(no_global_oom_handling))]
fn capacity_overflow() -> ! {
    panic!("capacity overflow");
}

#[track_caller]
#[inline(always)]
#[cfg(debug_assertions)]
unsafe fn assume(v: bool) {
    if !v {
        core::unreachable!()
    }
}
