use std::{
    marker::{PhantomData, PhantomPinned},
    pin::Pin,
    sync::Arc,
};

/**
 * A queue based on an intrusive list.
 */
pub(crate) struct Queue<T: Queueable> {
    head: QueueEntry,
    _phantom: PhantomData<T>,
}

impl<T: Queueable> Queue<T> {
    /* safety: caller must init() queue after it is pinned */
    pub(crate) const unsafe fn new() -> Self {
        Self {
            head: QueueEntry::new(),
            _phantom: PhantomData,
        }
    }

    /* safety: init must be called once after the queue is pinned */
    pub(crate) const unsafe fn init(self: Pin<&mut Self>) {
        let s = unsafe { &mut self.get_unchecked_mut().head };
        s.prev = std::ptr::from_mut(s);
        s.next = std::ptr::from_mut(s);
    }

    /**
     * Push an entry onto the queue.
     *
     * If the entry is already queued it will be unqueued and added to this
     * queue.
     * - If the entry is not queued: Add to this queue and hold reference.
     * - If the entry is queued on another queue: Remove from other queue
     *   and add to this queue, without changing reference count.
     * - If the entry is queued on this queue: remove from queue and add to
     *   end of queue, without changing reference count.
     */
    pub(crate) fn push(mut self: Pin<&mut Self>, entry: Pin<Arc<T>>) {
        let outer = std::ptr::from_ref(entry.as_ref().get_ref());
        let was_queued = entry.with_entry(|mut e| {
            let was_queued = e.is_queued();
            e.as_mut().unqueue();
            unsafe {
                let s = self.as_mut().get_unchecked_mut();
                let e = e.get_unchecked_mut();
                e.next = s.head.next;
                e.prev = &raw mut s.head;
                (*s.head.next).prev = e;
                s.head.next = e;
                e.outer = outer.cast::<()>();
            }
            was_queued
        });

        if !was_queued {
            /* reference now belongs to the list */
            let _ = Arc::into_raw(unsafe { Pin::into_inner_unchecked(entry) });
        }
    }

    /**
     * Pop an entry from the queue.
     *
     * Returns None if the queue is empty.
     *
     * Returns ownership of the entry to the caller.
     */
    pub(crate) fn pop(self: Pin<&mut Self>) -> Option<Pin<Arc<T>>> {
        (!self.is_empty()).then(|| {
            /* get last entry */
            let mut entry = unsafe { Pin::new_unchecked(&mut *self.head.prev) };
            /* remove it from the list */
            entry.as_mut().unqueue();
            /* reconstruct reference */
            unsafe { Pin::new_unchecked(Arc::from_raw(entry.outer.cast::<T>())) }
        })
    }

    /**
     * Release a queued entry.
     *
     * If the entry was queued on any queue, unqueue and release ownership
     * from the queue.
     */
    pub(crate) fn release(self: Pin<&mut Self>, entry: Pin<Arc<T>>) {
        entry.as_ref().with_entry(|mut e| {
            if e.is_queued() {
                /* reconstruct then drop reference */
                let _ = unsafe { Pin::new_unchecked(Arc::from_raw(e.outer.cast::<T>())) };
            }
            e.as_mut().unqueue();
        });
    }

    /**
     * Test if queue is empty.
     */
    pub(crate) fn is_empty(&self) -> bool {
        std::ptr::eq(self.head.prev, &raw const self.head)
    }
}

impl<T: Queueable> Drop for Queue<T> {
    fn drop(&mut self) {
        let mut this = unsafe { Pin::new_unchecked(self) };
        while this.as_mut().pop().is_some() {}
    }
}

#[derive(Debug)]
pub(crate) struct QueueEntry {
    prev: *mut QueueEntry,
    next: *mut QueueEntry,
    /* TODO: we only need to store an outer pointer because we have a boxed
     * value inside the Arc. When we ThinArc we can remove this. */
    outer: *const (),
    _pinned: PhantomPinned,
}

impl QueueEntry {
    pub(crate) const fn new() -> Self {
        Self {
            prev: std::ptr::null_mut(),
            next: std::ptr::null_mut(),
            outer: std::ptr::null_mut(),
            _pinned: PhantomPinned,
        }
    }

    fn unqueue(self: Pin<&mut Self>) {
        if !self.is_queued() {
            return;
        }
        unsafe {
            let s = self.get_unchecked_mut();
            (*s.prev).next = s.next;
            (*s.next).prev = s.prev;
            s.prev = std::ptr::null_mut();
        }
    }

    /**
     * Test if entry is queued.
     */
    const fn is_queued(&self) -> bool {
        !self.prev.is_null()
    }
}

pub(crate) trait Queueable {
    fn with_entry<R, F: FnMut(Pin<&mut QueueEntry>) -> R>(self: &Pin<Arc<Self>>, f: F) -> R;
}
