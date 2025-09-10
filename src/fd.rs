use crate::{
    EpollFlags,
    executor::{EXECUTOR, EpollWaker},
};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::{
    cell::RefCell,
    io::{Error, ErrorKind},
    os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd},
    pin::Pin,
    task::{Poll, ready},
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
        /* we set all file descriptors of interest as non-blocking -- this isn't
         * strictly necessary but it means that if a programmer accidentally
         * uses a file descriptor which isn't ready their program will return an
         * error rather than unexpectedly block */
        let flags = OFlag::from_bits_retain(fcntl(fd, FcntlArg::F_GETFL)?);
        fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        let s = Self {
            inner,
            ew: Box::pin(RefCell::new(EpollWaker::default())),
        };
        EXECUTOR.with(|e| e.epoll_add(fd, events, s.ew.as_ref()))?;
        Ok(s)
    }

    pub const fn ready(&mut self) -> FdFuture<'_, T> {
        FdFuture { fd: self }
    }

    pub fn poll_ready(&self, cx: &std::task::Context<'_>) -> Poll<EpollFlags> {
        self.ew.borrow_mut().poll(cx)
    }

    pub const fn with<Output, F: FnMut(&mut T, EpollFlags) -> Result<Output, Error>>(
        &mut self,
        func: F,
    ) -> FdWithFuture<'_, T, Output, F> {
        FdWithFuture { fd: self, func }
    }
}

impl<T: AsRawFd> Drop for Fd<T> {
    fn drop(&mut self) {
        /* safety: fd stays open for the duration of the borrow */
        let fd = unsafe { BorrowedFd::borrow_raw(self.inner.as_raw_fd()) };
        EXECUTOR.with(|e| e.epoll_del(fd)).unwrap();
    }
}

impl<T: AsRawFd> std::ops::Deref for Fd<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: AsRawFd> std::ops::DerefMut for Fd<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/*
 * FdFuture
 */
pub struct FdFuture<'a, T: AsRawFd> {
    fd: &'a mut Fd<T>,
}

impl<T: AsRawFd> Future for FdFuture<'_, T> {
    type Output = EpollFlags;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.fd.ew.borrow_mut().poll(cx)
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
 * FdWithFuture
 */
pub struct FdWithFuture<
    'a,
    T: AsRawFd,
    Output,
    F: FnMut(&mut T, EpollFlags) -> Result<Output, Error>,
> {
    fd: &'a mut Fd<T>,
    func: F,
}

impl<T: AsRawFd, Output, F: FnMut(&mut T, EpollFlags) -> Result<Output, Error> + Unpin> Future
    for FdWithFuture<'_, T, Output, F>
{
    type Output = Result<Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let events = ready!(self.fd.poll_ready(cx));
        let Self { fd, func, .. } = self.get_mut();
        match func(fd, events) {
            Err(e) if e.kind() == ErrorKind::WouldBlock => Poll::Pending,
            ret => Poll::Ready(ret),
        }
    }
}
