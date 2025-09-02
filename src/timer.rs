use crate::{Events, Fd, map_libc_fd, map_libc_result};
use std::{
    future::Future,
    io::Error,
    marker::PhantomData,
    os::fd::{AsRawFd, OwnedFd},
    pin::Pin,
    task::Poll,
};

/*
 * Timer
 */
pub struct Timer {
    fd: Fd<OwnedFd>,
    _not_send_not_sync: PhantomData<*mut ()>,
}

impl Timer {
    pub fn new() -> Result<Self, Error> {
        /* safety: we check return value before using it */
        let fd =
            map_libc_fd(unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) })?;
        Ok(Self {
            fd: Fd::new(fd, Events::IN)?,
            _not_send_not_sync: PhantomData,
        })
    }

    pub fn set_time(
        &mut self,
        initial: std::time::Duration,
        interval: Option<std::time::Duration>,
    ) -> Result<(), Error> {
        let new_value = libc::itimerspec {
            it_interval: interval.map_or(
                libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                },
                duration_to_timespec,
            ),
            it_value: duration_to_timespec(initial),
        };
        /* safety: we check the return value */
        map_libc_result(unsafe {
            libc::timerfd_settime(
                self.fd.as_raw_fd(),
                0,
                &raw const new_value,
                std::ptr::null_mut(),
            )
        })?;
        Ok(())
    }

    #[must_use]
    pub const fn tick(&mut self) -> TimerFuture {
        TimerFuture { timer: self }
    }
}

/*
 * TimerFuture
 */
pub struct TimerFuture<'a> {
    timer: &'a mut Timer,
}

impl Future for TimerFuture<'_> {
    type Output = Result<u64, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let timer_ready = std::pin::pin!(self.timer.fd.ready());
        if timer_ready.poll(cx) == Poll::Pending {
            return Poll::Pending;
        }

        let mut buf = std::mem::MaybeUninit::<[u8; 8]>::uninit();
        /* safety: we check the return value */
        map_libc_result(unsafe {
            libc::read(
                self.timer.fd.as_raw_fd(),
                buf.assume_init_mut().as_mut_ptr().cast(),
                buf.assume_init_ref().len(),
            ) as i32
        })?;
        /* safety: if read() does not return an error, buf is initialised */
        Poll::Ready(Ok(u64::from_ne_bytes(unsafe { *buf.assume_init_ref() })))
    }
}

/*
 * duration_to_timespec
 */
fn duration_to_timespec(duration: std::time::Duration) -> libc::timespec {
    libc::timespec {
        tv_sec: duration.as_secs().try_into().unwrap(),
        tv_nsec: duration.subsec_nanos().into(),
    }
}
