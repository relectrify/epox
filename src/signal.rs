pub use nix::sys::signal::{SigSet, Signal};

use std::io::Error;

pub struct SignalHandler {
    fd: crate::fd::Fd<nix::sys::signalfd::SignalFd>,
    _not_send_not_sync: core::marker::PhantomData<*mut ()>,
}

impl SignalHandler {
    pub fn new(sigset: SigSet) -> Result<Self, Error> {
        // register sigset as signalfd
        let signalfd = nix::sys::signalfd::SignalFd::with_flags(
            &sigset,
            nix::sys::signalfd::SfdFlags::SFD_NONBLOCK | nix::sys::signalfd::SfdFlags::SFD_CLOEXEC,
        )?;

        // ...then mask the signal so we can handle it
        // todo: we should unmask on drop?
        nix::sys::signal::sigprocmask(
            nix::sys::signal::SigmaskHow::SIG_BLOCK,
            Some(&sigset),
            None,
        )?;

        let fd = crate::fd::Fd::new(signalfd, nix::sys::epoll::EpollFlags::EPOLLIN)?;

        Ok(Self {
            fd,
            _not_send_not_sync: core::marker::PhantomData,
        })
    }

    pub async fn received(&mut self) -> Result<Option<nix::libc::signalfd_siginfo>, Error> {
        Ok(self
            .fd
            .with_fd(crate::EpollFlags::EPOLLIN, |fd| fd.read_signal())
            .await?)
    }
}
