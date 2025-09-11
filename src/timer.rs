use crate::{
    EpollFlags,
    fd::{AsFdWrapper, Fd},
};
pub use nix::sys::timerfd::Expiration;
use nix::sys::timerfd::{ClockId, TimerFd, TimerFlags, TimerSetTimeFlags};
use std::{io::Error, os::fd::AsFd};

/*
 * Timer
 */
pub struct Timer {
    fd: Fd<AsFdWrapper<TimerFd>>,
}

impl Timer {
    pub fn new() -> Result<Self, Error> {
        let fd = TimerFd::new(ClockId::CLOCK_MONOTONIC, TimerFlags::TFD_CLOEXEC)?;
        Ok(Self {
            fd: Fd::new(AsFdWrapper::new(fd), EpollFlags::EPOLLIN)?,
        })
    }

    pub fn set(&mut self, expiration: Expiration) -> Result<(), Error> {
        self.fd.set(expiration, TimerSetTimeFlags::empty())?;
        Ok(())
    }

    pub async fn tick(&mut self) -> Result<u64, Error> {
        self.fd
            .with(|fd, _events| {
                let mut buf = std::mem::MaybeUninit::<u64>::uninit();
                /* safety: read does not require an initialised buffer */
                nix::unistd::read(fd.as_fd(), unsafe {
                    std::slice::from_raw_parts_mut(
                        buf.as_mut_ptr().cast::<u8>(),
                        std::mem::size_of_val(&buf),
                    )
                })?;
                /* safety: if read() does not return an error, buf is initialised */
                Ok(unsafe { buf.assume_init() })
            })
            .await
    }
}
