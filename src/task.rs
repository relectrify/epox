use crate::{
    Priority,
    executor::{AnyTask, TaskRef, exec},
    queue::QueueEntry,
};
use pin_project::pin_project;
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
 * Why repr(C)? So that the queue_entry, priority and waker members are at
 * the same offset for all tasks, meaning that the generated implementations
 * of common functions can be shared between all instances of Task.
 */
#[pin_project]
#[repr(C)]
pub struct Task<T, F: Future<Output = T>> {
    #[pin]
    queue_entry: QueueEntry,
    priority: Priority,
    /* waker for Handle which may be waiting on this task */
    handle_waker: Cell<Waker>,
    #[pin]
    future: F,
    result: Cell<Option<T>>,
    _not_send_not_sync: PhantomData<*mut ()>,
}

impl<T, F: Future<Output = T>> Task<T, F> {
    pub fn new(future: F, priority: Priority) -> Self {
        Self {
            queue_entry: QueueEntry::new(),
            priority,
            handle_waker: Cell::new(Waker::noop().clone()),
            future,
            result: Cell::new(None),
            _not_send_not_sync: PhantomData,
        }
    }

    fn future(self: Pin<&mut Self>) -> Pin<&mut F> {
        self.project().future
    }

    fn ready(self: Pin<&mut Self>, result: T) {
        self.result.set(Some(result));
        self.handle_waker
            .replace(Waker::noop().clone())
            .wake_by_ref();
    }

    fn result(&self) -> Option<T> {
        self.result.take()
    }

    fn set_handle_waker(&self, waker: Waker) {
        self.handle_waker.set(waker);
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

    fn queue_entry(self: Pin<&mut Self>) -> Pin<&mut QueueEntry> {
        self.project().queue_entry
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/**
 * A handle to a task.
 *
 * Used to get the result (return value) of a task.
 */
pub struct Handle<T, F> {
    taskref: TaskRef,
    _phantom_t: PhantomData<T>,
    _phantom_f: PhantomData<F>,
}

impl<T: 'static, F: Future<Output = T> + 'static> Handle<T, F> {
    pub(crate) const fn new(taskref: TaskRef) -> Self {
        Self {
            taskref,
            _phantom_t: PhantomData,
            _phantom_f: PhantomData,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<T> {
        let r = self.taskref.task.borrow();
        let task: std::cell::Ref<'_, Task<T, F>> =
            std::cell::Ref::map(r, |t| t.as_any().downcast_ref::<Task<T, F>>().unwrap());
        task.result()
    }
}

impl<T: 'static, F: Future<Output = T> + 'static> Future for Handle<T, F> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let r = self.taskref.task.borrow();
        let task: std::cell::Ref<'_, Task<T, F>> =
            std::cell::Ref::map(r, |t| t.as_any().downcast_ref::<Task<T, F>>().unwrap());

        if let Some(result) = task.result() {
            Poll::Ready(result)
        } else {
            task.set_handle_waker(cx.waker().clone());
            Poll::Pending
        }
    }
}

/**
 * A future which yields once then continues running.
 */
pub struct YieldFuture {
    yielded: bool,
}

impl YieldFuture {
    #[must_use]
    pub const fn new() -> Self {
        Self { yielded: false }
    }
}

impl Future for YieldFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.get_mut().yielded = true;
            exec(|e| e.yield_now());
            Poll::Pending
        }
    }
}

/**
 * Yield control to the executor.
 *
 * The executor will check for epoll events, then poll all tasks of
 * equal or higher priority before polling the calling task again.
 */
pub async fn yield_now() {
    YieldFuture::new().await;
}
