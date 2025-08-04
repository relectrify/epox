mod executor;
mod task;
mod timer;

pub use task::Task;
pub use timer::Timer;

/**
 * Task priority.
 *
 * Tasks are run from high to low priority.
 */
#[derive(Copy, Clone)]
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
 * Map a libc `-1 means error is in errno` function to a [`Result`].
 *
 * Many C library functions return -1 on failure and store the reason for the
 * error in errno.
 */
fn map_libc_error(result: libc::c_int) -> Result<libc::c_int, std::io::Error> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result)
    }
}
