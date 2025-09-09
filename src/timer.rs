use crate::{EpollFlags, Fd, fd::AsFdWrapper};
pub use nix::sys::timerfd::Expiration;
use nix::sys::timerfd::{ClockId, TimerFd, TimerFlags, TimerSetTimeFlags};
use std::{io::Error, marker::PhantomData, os::fd::AsFd};

/*
 * Timer
 */
pub struct Timer {
    fd: Fd<AsFdWrapper<TimerFd>>,
    _not_send_not_sync: PhantomData<*mut ()>,
}

impl Timer {
    pub fn new() -> Result<Self, Error> {
        let fd = TimerFd::new(ClockId::CLOCK_MONOTONIC, TimerFlags::TFD_CLOEXEC)?;
        Ok(Self {
            fd: Fd::new(AsFdWrapper::new(fd), EpollFlags::EPOLLIN)?,
            _not_send_not_sync: PhantomData,
        })
    }

    pub fn set(&mut self, expiration: Expiration) -> Result<(), Error> {
        self.fd.set(expiration, TimerSetTimeFlags::empty())?;
        Ok(())
    }

    pub async fn tick(&mut self) -> Result<u64, Error> {
        self.fd
            .with(|fd, _events| {
                let mut buf = std::mem::MaybeUninit::<[u8; 8]>::uninit();
                /* safety: read does not require an initialised buffer */
                nix::unistd::read(fd.as_fd(), unsafe { buf.assume_init_mut() })?;
                /* safety: if read() does not return an error, buf is initialised */
                Ok(u64::from_ne_bytes(unsafe { *buf.assume_init_ref() }))
            })
            .await
    }
}
