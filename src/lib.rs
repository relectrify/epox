pub mod prelude {
    pub use futures::prelude::*;
}

pub mod signal;

pub mod timer;

pub use nix::sys::epoll::EpollFlags;
pub use signal::AsyncSignal;

pub use fd::{Fd, ReadableFd, WritableFd};
pub use task::Task;
pub use timer::Timer;

mod executor;
mod fd;
mod task;

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
 * Yield control to the executor.
 *
 * The executor will check for epoll events, then poll all tasks of equal or
 * higher priority before polling the calling task again.
 */
pub async fn yield_now() {
    task::YieldFuture::new().await;
}
