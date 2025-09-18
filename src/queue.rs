use std::{
    marker::{PhantomData, PhantomPinned},
    pin::Pin,
    rc::Rc,
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

    pub(crate) fn push(mut self: Pin<&mut Self>, entry: Pin<Rc<T>>) {
        let outer = std::ptr::from_ref(entry.as_ref().get_ref());
        entry.as_ref().with_entry(|mut e| {
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
        });

        /* reference now belongs to the list */
        let _ = Rc::into_raw(unsafe { Pin::into_inner_unchecked(entry) });
    }

    pub(crate) fn pop(self: Pin<&mut Self>) -> Option<Pin<Rc<T>>> {
        (!self.is_empty()).then(|| {
            /* get last entry */
            let mut entry = unsafe { Pin::new_unchecked(&mut *self.head.prev) };
            /* remove it from the list */
            entry.as_mut().unqueue();
            /* reconstruct reference */
            unsafe { Pin::new_unchecked(Rc::from_raw(entry.outer.cast::<T>())) }
        })
    }

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
     * value inside the Rc. When we ThinRc we can remove this. */
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

    pub(crate) fn unqueue(self: Pin<&mut Self>) {
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

    pub(crate) const fn is_queued(&self) -> bool {
        !self.prev.is_null()
    }
}

pub(crate) trait Queueable {
    fn with_entry<F: FnMut(Pin<&mut QueueEntry>)>(self: Pin<&Self>, f: F);
}
