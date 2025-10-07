use crate::executor::{TaskRef, exec};
use pin_project_lite::pin_project;
use std::{
    any::Any,
    cell::{Cell, RefCell},
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

/**
 * A task stores a future.
 */
pub(crate) type Task = GenericTask<dyn AnyFuture>;

pin_project! {
    pub(crate) struct GenericTask<F>
    where
        F: AnyFuture,
        F: ?Sized,
    {
        _not_send_not_sync: PhantomData<*mut ()>,
        // The last field of a struct can hold a dynamically sized type.
        // Because F: ?Sized, GenericTask<F> can be coerced into GenericTask<dyn AnyFuture>.
        // see https://doc.rust-lang.org/nomicon/exotic-sizes.html#dynamically-sized-types-dsts
        #[pin]
        inner: F,
    }
}

impl Task {
    pub(crate) fn new_pinned_boxed_refcell<F: Future + 'static>(
        future: F,
    ) -> Pin<Box<RefCell<Self>>>
    where
        F::Output: 'static,
    {
        Box::pin(RefCell::new(GenericTask::<Inner<F>> {
            _not_send_not_sync: PhantomData,
            inner: Inner {
                output: Cell::new(None),
                future,
            },
        }))
    }

    pub(crate) fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.as_mut().project().inner.poll(cx)
    }
}

pub(crate) trait AnyFuture {
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()>;
    fn take_output(&self, any: &mut dyn Any);
}

pin_project! {
    pub(crate) struct Inner<F: Future> {
        output: Cell<Option<F::Output>>,
        #[pin]
        future: F,
    }
}

impl<F: Future> AnyFuture for Inner<F>
where
    F::Output: 'static,
{
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.project();
        match this.future.poll(cx) {
            Poll::Ready(ret) => {
                this.output.set(Some(ret));
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }

    /// `any` must be an [`Option<F::Output>`]
    ///
    /// This stops F::Output from bubbling up through the [`AnyFuture`] trait
    fn take_output(&self, any: &mut dyn Any) {
        let x = any.downcast_mut::<Option<F::Output>>().unwrap();
        *x = self.output.take();
    }
}

/**
 * A handle to a task.
 *
 * Used to get the result (return value) of a task.
 */
pub struct Handle<T> {
    taskref: TaskRef,
    _phantom_t: PhantomData<T>,
}

impl<T: 'static> Handle<T> {
    pub(crate) const fn new(taskref: TaskRef) -> Self {
        Self {
            taskref,
            _phantom_t: PhantomData,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<T> {
        let mut ret = None;
        self.taskref.task.borrow().inner.take_output(&mut ret);
        ret
    }

    pub fn abort(&self) {
        exec(|e| e.abort(self.taskref.clone()));
    }
}

impl<T: 'static> Future for Handle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(result) = self.result() {
            Poll::Ready(result)
        } else {
            self.taskref.handle_waker.set(cx.waker().clone());
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

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.get_mut().yielded = true;
            // waking this task will add it to the back of the queue
            // see https://doc.rust-lang.org/std/task/struct.Waker.html#method.wake
            cx.waker().wake_by_ref();
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
