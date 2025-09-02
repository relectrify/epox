use crate::{
    Priority,
    executor::{AnyTask, TaskRef},
};
use std::{
    any::Any,
    cell::Cell,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, Waker},
};

/**
 * A task stores a future.
 *
 * Why repr(C)? So that the pending flag is at the same offset for all
 * tasks, meaning that the generated implementations of {set,take}_pending
 * can be shared between all instances of Task.
 */
#[repr(C)]
pub struct Task<T, F: Future<Output = T>> {
    pending: bool,
    priority: Priority,
    waker: Cell<Waker>,
    future: F,
    result: Cell<Option<T>>,
    _not_send_not_sync: PhantomData<*mut ()>,
}

impl<T, F: Future<Output = T>> Task<T, F> {
    pub(crate) fn new(future: F, priority: Priority) -> Self {
        Self {
            pending: true,
            priority,
            waker: Cell::new(Waker::noop().clone()),
            future,
            result: Cell::new(None),
            _not_send_not_sync: PhantomData,
        }
    }

    fn future(self: Pin<&mut Self>) -> Pin<&mut F> {
        /* safety: self is pinned therefore self.future is pinned */
        unsafe { self.map_unchecked_mut(|s| &mut s.future) }
    }

    fn ready(self: Pin<&mut Self>, result: T) {
        self.result.set(Some(result));
        self.waker.replace(Waker::noop().clone()).wake_by_ref();
    }

    fn result(&self) -> Option<T> {
        self.result.take()
    }

    fn set_waker(&self, waker: Waker) {
        self.waker.set(waker);
    }
}

impl<T: 'static, F: Future<Output = T> + 'static> AnyTask for Task<T, F> {
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match self.as_mut().future().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.as_mut().ready(result);
                Poll::Ready(())
            }
        }
    }

    fn priority(&self) -> Priority {
        self.priority
    }

    fn set_pending(&mut self) {
        self.pending = true;
    }

    fn take_pending(&mut self) -> bool {
        std::mem::replace(&mut self.pending, false)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/*
 * Handle
 */
pub struct Handle<T, F> {
    task: TaskRef,
    _phantom_t: PhantomData<T>,
    _phantom_f: PhantomData<F>,
}

impl<T: 'static, F: Future<Output = T> + 'static> Handle<T, F> {
    pub(crate) fn new(task: TaskRef) -> Self {
        Self {
            task,
            _phantom_t: PhantomData,
            _phantom_f: PhantomData,
        }
    }

    pub fn result(&self) -> Option<T> {
        let r = self.task.borrow();
        let task: std::cell::Ref<'_, Task<T, F>> =
            std::cell::Ref::map(r, |t| t.as_any().downcast_ref::<Task<T, F>>().unwrap());
        task.result()
    }
}

impl<T: 'static, F: Future<Output = T> + 'static> Future for Handle<T, F> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let r = self.task.borrow();
        let task: std::cell::Ref<'_, Task<T, F>> =
            std::cell::Ref::map(r, |t| t.as_any().downcast_ref::<Task<T, F>>().unwrap());

        if let Some(result) = task.result() {
            Poll::Ready(result)
        } else {
            task.set_waker(cx.waker().clone());
            Poll::Pending
        }
    }
}
