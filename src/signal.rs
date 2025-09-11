use crate::Fd;
pub use nix::sys::signal::{SigSet, Signal};
use nix::sys::{
    signal::{SigmaskHow, sigprocmask},
    signalfd::{SfdFlags, SignalFd},
};
use std::{io::Error, os::fd::AsFd};

pub struct AsyncSignal {
    fd: Fd<SignalFd>,
    sigset: SigSet,
}

/**
 * Handle signals asynchronously.
 *
 * Receiving the same signal with multiple Signal instances is not
 * supported.
 *
 * Attempting to receive a signal which is blocked will result in an error.
 */
impl AsyncSignal {
    pub fn new(sigset: SigSet) -> Result<Self, Error> {
        if sigset == SigSet::empty() {
            return Err(Error::other("empty sigset"));
        }

        /* make sure signals we're trying to receive are not blocked */
        let mut oldset = SigSet::empty();
        nix::sys::signal::sigprocmask(SigmaskHow::SIG_SETMASK, None, Some(&mut oldset))?;
        for s in &sigset {
            if oldset.contains(s) {
                return Err(Error::other("signal is blocked"));
            }
        }

        let signalfd = SignalFd::with_flags(&sigset, SfdFlags::SFD_CLOEXEC)?;

        /* signals received by signalfd need to be blocked to prevent them
         * from being handled according to their default dispositions */
        nix::sys::signal::sigprocmask(SigmaskHow::SIG_BLOCK, Some(&sigset), None)?;

        Ok(Self {
            fd: Fd::new(signalfd, nix::sys::epoll::EpollFlags::EPOLLIN)?,
            sigset,
        })
    }

    pub async fn receive(&mut self) -> Result<nix::libc::signalfd_siginfo, Error> {
        self.fd
            .with(|fd, _events| {
                let mut buf = std::mem::MaybeUninit::<nix::libc::signalfd_siginfo>::uninit();
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

impl Drop for AsyncSignal {
    fn drop(&mut self) {
        /* unblock signals as they will no longer be received by signalfd */
        let _ = sigprocmask(SigmaskHow::SIG_UNBLOCK, Some(&self.sigset), None);
    }
}
