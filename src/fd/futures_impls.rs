use std::{
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    pin::Pin,
    task::Poll,
};

use super::{EpollFlags, Fd};

/// Wrapper that implements [`futures::AsyncRead`]
pub struct ReadableFd<T: AsRawFd> {
    inner: Fd<T>,
    _not_send_not_sync: core::marker::PhantomData<*mut ()>,
}

impl<T: AsRawFd> ReadableFd<T> {
    pub fn new(inner: T) -> Result<Self, std::io::Error> {
        let inner = Fd::new(inner, EpollFlags::EPOLLIN)?;
        Ok(Self {
            inner,
            _not_send_not_sync: core::marker::PhantomData,
        })
    }
}

impl<T: AsRawFd> futures::AsyncRead for ReadableFd<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if self
            .inner
            .poll_ready(cx)
            .intersection(EpollFlags::EPOLLIN)
            .is_empty()
        {
            return Poll::Pending;
        }

        /* safety: we already know the fd is valid */
        let num_read = nix::unistd::read(
            unsafe { BorrowedFd::borrow_raw(self.inner.as_raw_fd()) },
            buf,
        )?;
        Poll::Ready(Ok(num_read))
    }
}

impl<T: AsRawFd> AsRawFd for ReadableFd<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

/// Wrapper that implements [`futures::AsyncWrite`]
pub struct WritableFd<T: AsRawFd> {
    inner: Fd<T>,
    _not_send_not_sync: core::marker::PhantomData<*mut ()>,
}

impl<T: AsRawFd> WritableFd<T> {
    pub fn new(inner: T) -> Result<Self, std::io::Error> {
        let inner = Fd::new(inner, EpollFlags::EPOLLOUT | EpollFlags::EPOLLET)?;
        Ok(Self {
            inner,
            _not_send_not_sync: core::marker::PhantomData,
        })
    }
}

impl<T: AsRawFd> futures::AsyncWrite for WritableFd<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self
            .inner
            .poll_ready(cx)
            .intersection(EpollFlags::EPOLLOUT)
            .is_empty()
        {
            return Poll::Pending;
        }

        /* safety: we already know the fd is valid */
        let num_wrote = nix::unistd::write(
            unsafe { BorrowedFd::borrow_raw(self.inner.as_raw_fd()) },
            buf,
        )?;
        Poll::Ready(Ok(num_wrote))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        // todo - this really just needs to check if there are any pending writes?
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        // i'm not sure what this is actually used for?
        todo!()
    }
}

impl<T: AsRawFd> AsRawFd for WritableFd<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}
