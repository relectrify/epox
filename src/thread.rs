//! Native threads that can be `.await`ed on by asynchronous tasks.
//!
//! # Examples
//!
//! ```rust
//! # epox::spawn(async {
//! let thread = epox::thread::spawn(|| {
//!     // do blocking work on a separate thread
//!     std::thread::sleep(std::time::Duration::from_secs(1));
//!     "value"
//! });
//!
//! // await result
//! // other tasks in the same executor can run while we wait
//! assert_eq!(thread.join().await.unwrap(), "value");
//! # });
//! # epox::run().unwrap();
//! ```

use std::{
    sync::{Arc, Mutex},
    task::{Poll, Waker},
};

/// Spawn a thread, returning a [`Handle`].
pub fn spawn<F, T>(f: F) -> Handle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // this .expect() is the same thing the stdlib does
    // see https://doc.rust-lang.org/std/thread/struct.Builder.html#method.spawn
    spawn_with_builder(std::thread::Builder::new(), f).expect("failed to spawn thread")
}

pub fn spawn_with_builder<F, T>(builder: std::thread::Builder, f: F) -> std::io::Result<Handle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let state = Arc::new(Mutex::new(State::Running(Waker::noop().clone())));

    let join_handle = builder.spawn({
        let state = state.clone();
        move || {
            let val = f();

            // update state before waking task
            let State::Running(waker) =
                std::mem::replace(&mut *state.lock().unwrap(), State::Complete)
            else {
                // this is the only place we change state from running
                panic!("thread finished but state was already Complete");
            };
            waker.wake();
            val
        }
    })?;

    Ok(Handle {
        state,
        join_handle: Some(join_handle),
    })
}

/// Handle to a thread.
///
/// Similar to a [`JoinHandle`], but [`join()`] returns a [`Future`] to await on
/// the result of the spawned thread without blocking the executor on the
/// current thread.
///
/// Like a `JoinHandle`, the spawned thread is detached and continues running
/// if this handle is dropped.
///
/// [`JoinHandle`]: std::thread::JoinHandle
/// [`join()`]: Self::join
pub struct Handle<T> {
    state: Arc<Mutex<State>>,
    join_handle: Option<std::thread::JoinHandle<T>>,
}

enum State {
    Running(Waker),
    Complete,
}

impl<T> Handle<T> {
    #[must_use]
    pub fn is_finished(&self) -> bool {
        // unwrap: join_handle will be Some until the JoinFuture returns Poll::Ready,
        // and creating the JoinFuture takes the Handle by value
        self.join_handle.as_ref().unwrap().is_finished()
    }

    pub const fn join(self) -> JoinFuture<T> {
        JoinFuture { handle: self }
    }

    #[must_use]
    pub fn thread(&self) -> &std::thread::Thread {
        // unwrap: join_handle will be Some until the JoinFuture returns Poll::Ready,
        // and creating the JoinFuture takes the Handle by value
        self.join_handle.as_ref().unwrap().thread()
    }
}

/// Future for [`Handle::join()`].
#[must_use]
pub struct JoinFuture<T> {
    handle: Handle<T>,
}

impl<T> Future for JoinFuture<T> {
    /// A [`std::thread::Result`].
    type Output = std::thread::Result<T>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        let handle = &mut self.handle;

        match &mut *handle.state.lock().unwrap() {
            State::Running(waker) => {
                waker.clone_from(cx.waker());
                Poll::Pending
            }
            State::Complete => Poll::Ready(
                handle
                    .join_handle
                    .take()
                    .expect("future polled after completion")
                    .join(),
            ),
        }
    }
}
