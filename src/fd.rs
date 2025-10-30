use crate::{
    EpollFlags,
    executor::{EpollWaker, exec},
};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::{
    cell::RefCell,
    io::Error,
    os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd},
    pin::Pin,
    task::Poll,
};

/**
 * A file descriptor of interest.
 *
 * Can wrap anything which implements AsRawFd.
 */
pub struct Fd<T: AsRawFd> {
    inner: T,
    ew: Pin<Box<RefCell<EpollWaker>>>,
}

impl<T: AsRawFd> Fd<T> {
    pub fn new(inner: T, events: EpollFlags) -> Result<Self, Error> {
        /* safety: fd stays open for the duration of the borrow */
        let fd = unsafe { BorrowedFd::borrow_raw(inner.as_raw_fd()) };
        /* all file descriptors must be set as non-blocking as we need to
         * receive epoll events in edge triggered mode */
        let flags = OFlag::from_bits_retain(fcntl(fd, FcntlArg::F_GETFL)?);
        fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;

        /* assume we're ready to read and write to the fd - epoll(7) implies that we
         * won't see these event from an fd registered with EPOLLET until read or
         * write return EAGAIN */
        let waker_events = events & (EpollFlags::EPOLLIN | EpollFlags::EPOLLOUT);

        let s = Self {
            inner,
            ew: Box::pin(RefCell::new(EpollWaker::new(waker_events))),
        };
        /* all events must be edge triggered because tasks usually register
         * multiple file descriptors but only wait on one at a time */
        exec(|e| e.epoll_add(fd, events | EpollFlags::EPOLLET, s.ew.as_ref()))?;
        Ok(s)
    }

    pub fn poll_with_mut<Output, F: FnMut(&mut T, EpollFlags) -> Result<Output, Error>>(
        &mut self,
        cx: &std::task::Context<'_>,
        mut func: F,
    ) -> Poll<Result<Output, std::io::Error>> {
        let ret = self
            .ew
            .borrow_mut()
            .poll_with(cx, |events| func(&mut self.inner, events));

        match ret {
            crate::executor::ControlFlow::Normal(poll) => poll,
            crate::executor::ControlFlow::Yield => {
                // the docs say "yielding to competing tasks is not guaranteed", but we
                // know that we're running in epox which will let other tasks
                // run https://doc.rust-lang.org/std/task/struct.Waker.html#impl-Waker
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    pub const fn with_mut<Output, F: FnMut(&mut T, EpollFlags) -> Result<Output, Error>>(
        &mut self,
        func: F,
    ) -> FdWithMutFuture<'_, T, Output, F> {
        FdWithMutFuture { fd: self, func }
    }

    pub fn poll_with<Output, F: FnMut(&T, EpollFlags) -> Result<Output, Error>>(
        &self,
        cx: &std::task::Context<'_>,
        mut func: F,
    ) -> Poll<Result<Output, std::io::Error>> {
        let ret = self
            .ew
            .borrow_mut()
            .poll_with(cx, |events| func(&self.inner, events));

        match ret {
            crate::executor::ControlFlow::Normal(poll) => poll,
            crate::executor::ControlFlow::Yield => {
                // the docs say "yielding to competing tasks is not guaranteed", but we
                // know that we're running in epox which will let other tasks
                // run https://doc.rust-lang.org/std/task/struct.Waker.html#impl-Waker
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    pub const fn with<Output, F: FnMut(&T, EpollFlags) -> Result<Output, Error>>(
        &self,
        func: F,
    ) -> FdWithFuture<'_, T, Output, F> {
        FdWithFuture { fd: self, func }
    }

    pub const fn inner(&self) -> &T {
        &self.inner
    }

    pub const fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: AsRawFd> Drop for Fd<T> {
    fn drop(&mut self) {
        /* safety: fd stays open for the duration of the borrow */
        let fd = unsafe { BorrowedFd::borrow_raw(self.inner.as_raw_fd()) };
        // ignore errors - if we're being dropped after the epoll fd has closed this
        // will fail
        // this is expected and will happen after Executor::shutdown() is called
        let _ = exec(|e| e.epoll_del(fd));
    }
}

/**
 * A wrapper to adapt an object which implements AsFd and not AsRawFd.
 *
 * FIXME: this really feels like it could be improved.
 */
pub struct AsFdWrapper<T: AsFd> {
    inner: T,
}

impl<T: AsFd> AsFdWrapper<T> {
    pub const fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T: AsFd> AsRawFd for AsFdWrapper<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_fd().as_raw_fd()
    }
}

impl<T: AsFd> std::ops::Deref for AsFdWrapper<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: AsFd> std::ops::DerefMut for AsFdWrapper<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/*
 * FdWithMutFuture
 */
#[doc(hidden)]
pub struct FdWithMutFuture<
    'a,
    T: AsRawFd,
    Output,
    F: FnMut(&mut T, EpollFlags) -> Result<Output, Error>,
> {
    fd: &'a mut Fd<T>,
    func: F,
}

impl<T: AsRawFd, Output, F: FnMut(&mut T, EpollFlags) -> Result<Output, Error> + Unpin> Future
    for FdWithMutFuture<'_, T, Output, F>
{
    type Output = Result<Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let Self { fd, func, .. } = self.get_mut();
        fd.poll_with_mut(cx, func)
    }
}

/*
 * FdWithFuture
 */
#[doc(hidden)]
pub struct FdWithFuture<'a, T: AsRawFd, Output, F: FnMut(&T, EpollFlags) -> Result<Output, Error>> {
    fd: &'a Fd<T>,
    func: F,
}

impl<T: AsRawFd, Output, F: FnMut(&T, EpollFlags) -> Result<Output, Error> + Unpin> Future
    for FdWithFuture<'_, T, Output, F>
{
    type Output = Result<Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let Self { fd, func, .. } = self.get_mut();
        fd.poll_with(cx, func)
    }
}
