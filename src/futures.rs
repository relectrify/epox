use crate::{EpollFlags, Fd};
use futures_util::FutureExt;
use std::{
    io::{Error, ErrorKind},
    os::fd::{AsRawFd, BorrowedFd},
    pin::Pin,
    task::{Poll, ready},
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
        self: Pin<&mut Self>,
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
 *
 * [`poll_close()`] is unsupported and will always return an error.
 *
 * [`poll_close()`]: futures_io::AsyncWrite::poll_close()
 */
pub struct AsyncWriteFd<T: AsRawFd> {
    inner: Fd<T>,
    flush_handle: Option<crate::thread::JoinFuture<std::io::Result<()>>>,
}

impl<T: AsRawFd> AsyncWriteFd<T> {
    pub fn new(inner: T) -> Result<Self, std::io::Error> {
        Ok(Self {
            inner: Fd::new(inner, EpollFlags::EPOLLOUT | EpollFlags::EPOLLET)?,
            flush_handle: None,
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
        self: Pin<&mut Self>,
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
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(self.inner.inner().as_raw_fd()) };

        let flush_handle = self.flush_handle.get_or_insert_with(|| {
            let fd = borrowed_fd.try_clone_to_owned();
            crate::thread::spawn(|| {
                let fd = fd?;
                match nix::unistd::isatty(&fd) {
                    Ok(true) => {
                        nix::sys::termios::tcdrain(fd)?;
                    }
                    Ok(false) => {
                        nix::unistd::fsync(fd)?;
                    }
                    Err(e) => return Err(e.into()),
                }
                Ok(())
            })
            .join()
        });

        // JoinFuture::poll will return Err if the spawned thread panicked - unwrap will
        // continue that panic on this thread
        let ret = ready!(flush_handle.poll_unpin(cx)).unwrap();
        self.flush_handle = None;
        Poll::Ready(ret)
    }

    /// Not supported - will always return an error.
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
