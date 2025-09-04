use std::{
    pin::Pin,
    task::{Context, Poll},
};

use crate::executor::EXECUTOR;

pub const fn yield_now() -> AsyncYield {
    AsyncYield { has_yielded: false }
}

#[must_use]
pub struct AsyncYield {
    has_yielded: bool,
}

impl Future for AsyncYield {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.has_yielded {
            Poll::Ready(())
        } else {
            self.get_mut().has_yielded = true;
            EXECUTOR.with(|e| e.yield_task(cx));
            Poll::Pending
        }
    }
}
