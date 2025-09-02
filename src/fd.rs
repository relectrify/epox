use crate::{
    Events,
    executor::{EXECUTOR, EpollWaker},
    map_libc_result,
};
use std::{cell::RefCell, os::fd::AsRawFd, pin::Pin, task::Poll};

/**
 * A file descriptor of interest.
 */
pub struct Fd<T: AsRawFd> {
    inner: T,
    ew: Pin<Box<RefCell<EpollWaker>>>,
}

impl<T: AsRawFd> Fd<T> {
    pub fn new(inner: T, events: Events) -> Result<Self, std::io::Error> {
        /* we set all file descriptors of interest as non-blocking -- this isn't
         * strictly necessary but it means that if a programmer accidentally
         * uses a file descriptor which isn't ready their program will return an
         * error rather than unexpectedly block */
        let flags = map_libc_result(unsafe { libc::fcntl(inner.as_raw_fd(), libc::F_GETFL) })?;
        map_libc_result(unsafe {
            libc::fcntl(inner.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
        })?;
        let s = Self {
            inner,
            ew: Box::pin(RefCell::new(EpollWaker::default())),
        };
        EXECUTOR.with(|e| e.epoll_add(s.inner.as_raw_fd(), events, s.ew.as_ref()))?;
        Ok(s)
    }

    pub const fn ready(&mut self) -> FdFuture<T> {
        FdFuture { fd: self }
    }
}

impl<T: AsRawFd> Drop for Fd<T> {
    fn drop(&mut self) {
        EXECUTOR
            .with(|e| e.epoll_del(self.inner.as_raw_fd()))
            .unwrap();
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
    type Output = Events;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        const EMPTY: Events = Events::empty();
        match self.fd.ew.borrow_mut().poll(cx) {
            EMPTY => Poll::Pending,
            events => Poll::Ready(events),
        }
    }
}
