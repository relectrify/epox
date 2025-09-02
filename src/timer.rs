use std::os::fd::{AsRawFd, OwnedFd};
use std::{cell::RefCell, io::Error, marker::PhantomData, pin::Pin, rc::Rc, task::Poll};

use crate::executor::{EXECUTOR, EpollWaker};
use crate::{map_libc_fd, map_libc_result};

/*
 * Timer
 */
pub struct Timer {
    fd: OwnedFd,
    ew: Pin<Rc<RefCell<EpollWaker>>>,
    _not_send_not_sync: PhantomData<*mut ()>,
}

impl Timer {
    pub fn new() -> Result<Self, Error> {
        /* safety: we check return value before using it */
        let fd = map_libc_fd(unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
            )
        })?;
        let s = Self {
            fd,
            ew: Rc::pin(RefCell::new(EpollWaker::default())),
            _not_send_not_sync: PhantomData,
        };
        EXECUTOR.with(|e| e.epoll_add(&s.fd, libc::EPOLLIN, s.ew.clone()))?;
        Ok(s)
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
}

impl Drop for Timer {
    fn drop(&mut self) {
        EXECUTOR.with(|e| e.epoll_del(&self.fd)).unwrap();
    }
}

impl Future for Timer {
    type Output = Result<u64, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if !self.ew.borrow_mut().poll(cx) {
            return Poll::Pending;
        }

        let mut buf = std::mem::MaybeUninit::<[u8; 8]>::uninit();
        /* safety: we check the return value */
        map_libc_result(unsafe {
            libc::read(
                self.fd.as_raw_fd(),
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
