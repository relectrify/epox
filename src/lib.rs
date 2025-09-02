mod executor;
mod task;
mod timer;

use std::os::fd::{FromRawFd, OwnedFd};
pub use task::Task;
pub use timer::Timer;

/**
 * Task priority.
 *
 * Tasks are run from high to low priority.
 */
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    High,
    Normal,
    Low,
}

bitflags::bitflags! {
    /**
     * Epoll events.
     */
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct Events: u32 {
        /** The associated file is available for read(2) operations. */
        const IN = libc::EPOLLIN as u32;
        /** The associated file is available for write(2) operations. */
        const OUT = libc::EPOLLOUT as u32;
        /** Stream socket peer closed connection, or shut down writing half of connection. */
        const RDHUP = libc::EPOLLRDHUP as u32;
        /** There is an exceptional condition on the file descriptor. */
        const PRI = libc::EPOLLPRI as u32;
        /** Error condition happened on the associated file descriptor. */
        const ERR = libc::EPOLLERR as u32;
        /** Hang up happened on the associated file descriptor. */
        const HUP = libc::EPOLLHUP as u32;
    }
}

/**
 * Spawn a task on the thread local executor with normal priority.
 */
pub fn spawn<T: 'static, F: Future<Output = T> + 'static>(future: F) -> task::Handle<T, F> {
    executor::EXECUTOR.with(|e| e.spawn(future, Priority::Normal))
}

/**
 * Spawn a task on the thread local executor with [`Priority`] priority.
 */
pub fn spawn_with_priority<T: 'static, F: Future<Output = T> + 'static>(
    future: F,
    priority: Priority,
) -> task::Handle<T, F> {
    executor::EXECUTOR.with(|e| e.spawn(future, priority))
}

/**
 * Run the thread local executor until no futures are pending.
 */
pub fn run() -> Result<(), std::io::Error> {
    executor::EXECUTOR.with(|e| e.run())
}

/**
 * Map a libc `-1 means error is in errno` function returning a file
 * descriptor to a [`Result`].
 *
 * Many C library functions return -1 on failure and store the reason for
 * the error in errno.
 */
fn map_libc_fd(result: libc::c_int) -> Result<OwnedFd, std::io::Error> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        /* safety: result has been checked for validity */
        Ok(unsafe { OwnedFd::from_raw_fd(result) })
    }
}

/**
 * Map a libc `-1 means error is in errno` function returning an integer to
 * a [`Result`].
 *
 * Many C library functions return -1 on failure and store the reason for
 * the error in errno.
 */
fn map_libc_result(result: libc::c_int) -> Result<libc::c_int, std::io::Error> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result)
    }
}
