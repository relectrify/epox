use crate::Timer;
use futures_util::FutureExt;
use pin_project_lite::pin_project;
use std::{task::Poll, time::Duration};

/// Run the inner [`Future`] until it completes, or `duration` elapses -
/// whichever comes first
pub fn timeout<F: Future>(duration: Duration, future: F) -> std::io::Result<Timeout<F>> {
    let mut timer = Timer::new()?;
    timer.set(nix::sys::timer::Expiration::OneShot(duration.into()))?;
    Ok(Timeout { future, timer })
}

pin_project! {
    pub struct Timeout<F: Future> {
        #[pin]
        future: F,
        timer: Timer,
    }
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

/// Sleeps for `duration` before returning
pub async fn sleep(duration: Duration) -> std::io::Result<()> {
    let mut timer = Timer::new()?;
    timer.set(nix::sys::timer::Expiration::OneShot(duration.into()))?;
    timer.tick().await?;
    Ok(())
}
