use crate::{EpollFlags, Fd};
use std::{
    io::{Error, ErrorKind},
    os::fd::{AsRawFd, BorrowedFd},
    pin::Pin,
    task::Poll,
};

/**
 * Wrapper which implements [`futures_io::AsyncRead`]
 */
pub struct AsyncReadFd<T: AsRawFd> {
    inner: Fd<T>,
}

impl<T: AsRawFd> AsyncReadFd<T> {
    pub fn new(inner: T) -> Result<Self, std::io::Error> {
        Ok(Self {
            inner: Fd::new(inner, EpollFlags::EPOLLIN)?,
        })
    }
}

impl<T: AsRawFd> std::ops::Deref for AsyncReadFd<T> {
    type Target = Fd<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: AsRawFd> std::ops::DerefMut for AsyncReadFd<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T: AsRawFd + Unpin> futures_io::AsyncRead for AsyncReadFd<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        self.inner.poll_with(cx, |fd, _events| {
            /* safety: we already know the fd is valid */
            Ok(nix::unistd::read(
                unsafe { BorrowedFd::borrow_raw(fd.as_raw_fd()) },
                buf,
            )?)
        })
    }
}

/**
 * Wrapper which implements [`futures_io::AsyncWrite`]
 */
pub struct AsyncWriteFd<T: AsRawFd> {
    inner: Fd<T>,
}

impl<T: AsRawFd> AsyncWriteFd<T> {
    pub fn new(inner: T) -> Result<Self, std::io::Error> {
        Ok(Self {
            inner: Fd::new(inner, EpollFlags::EPOLLOUT | EpollFlags::EPOLLET)?,
        })
    }
}

impl<T: AsRawFd> std::ops::Deref for AsyncWriteFd<T> {
    type Target = Fd<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: AsRawFd> std::ops::DerefMut for AsyncWriteFd<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T: AsRawFd + Unpin> futures_io::AsyncWrite for AsyncWriteFd<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.inner.poll_with(cx, |fd, _events| {
            /* safety: we already know the fd is valid */
            Ok(nix::unistd::write(
                unsafe { BorrowedFd::borrow_raw(fd.as_raw_fd()) },
                buf,
            )?)
        })
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        /*
         * TODO: asynchronous flush support.
         *
         * In Linux tcdrain() and fflush() are always blocking operations. To
         * implement this we need to:
         * - create a thread
         *   - in the thread, if isatty() { tcdrain() } else fflush()
         * - asynchronously wait for the thread to complete and pass through the
         *   result of the flush operation.
         *
         * For now, let's tell callers we can't do this.
         */
        Poll::Ready(Err(Error::new(
            ErrorKind::Unsupported,
            "epox doesn't support poll_flush yet",
        )))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        /*
         * TODO: asynchronous close support.
         *
         * In Linux, close is asynchronous if O_NONBLOCK is set. To implement
         * this we need to:
         * - determine if we are closing the last file descriptor referring to the
         *   file description in question (need to parse /proc/self/fd?)
         * - clear O_NONBLOCK on the file descriptor
         * - create a thread
         *   - call close() in the thread
         * - asynchronously wait for the thread to complete and pass through the
         *   result of the close() call.
         *
         * For now, let's tell callers we can't do this.
         */
        Poll::Ready(Err(Error::new(
            ErrorKind::Unsupported,
            "epox doesn't support poll_close yet",
        )))
    }
}
