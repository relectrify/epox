use futures_util::FutureExt;
use std::{task::Poll, time::Duration};

use crate::Timer;

/// Run the inner [`Future`] until it completes, or `duration` elapses - whichever comes first
pub fn timeout<F: Future>(duration: Duration, future: F) -> std::io::Result<Timeout<F>> {
    let mut timer = Timer::new()?;
    timer.set(nix::sys::timer::Expiration::OneShot(duration.into()))?;
    Ok(Timeout { future, timer })
}

#[pin_project::pin_project]
pub struct Timeout<F: Future> {
    #[pin]
    future: F,
    timer: Timer,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, ()>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Poll::Ready(ret) = this.future.poll(cx) {
            Poll::Ready(Ok(ret))
        } else if this.timer.tick().poll_unpin(cx).is_ready() {
            Poll::Ready(Err(()))
        } else {
            Poll::Pending
        }
    }
}
