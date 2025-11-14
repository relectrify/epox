use futures_lite::FutureExt;

#[test]
fn composable_tasks() {
    let t1 = epox::spawn(async {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(100).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();

        20
    });

    let t2 = epox::spawn(async {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(200).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();

        22
    });

    let t3 = epox::spawn(async { t1.await + t2.await });

    epox::run().unwrap();

    assert_eq!(t3.result().unwrap(), 42);
}

#[test]
fn unrelated_tasks() {
    let t1 = epox::spawn(async {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(100).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();

        20
    });

    // when t1 finishes, this will still be waiting in the kernel epoll object
    let t2 = epox::spawn(async {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(200).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();

        "value"
    });

    epox::run().unwrap();

    assert_eq!(t1.result().unwrap(), 20);
    assert_eq!(t2.result().unwrap(), "value");
}

#[test]
fn abort_task() {
    let t1 = epox::spawn(async {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(1000).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();
        panic!("this task should be aborted");
    });

    epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(100).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();
        t1.abort();
        22
    });

    epox::run().unwrap();
}

#[test]
fn wake_completed_task() {
    // this reproduces a bug:

    // a task might complete while still interested in some futures. if no other
    // tasks were to await these futures before they become ready, and the futures
    // were still around after the task completes, they may have attempted to wake
    // that task after it had returned Poll::Ready. if there was a TaskHandle bound
    // to that task, the task wouldn't have been dropped when it returned
    // Poll::Ready, so the waker would successfully put the task back in the run
    // queue. epox would then panic when it tried to poll the completed task again.

    // task to keep the executor alive after the other task finishes, for long
    // enough that the other timer tries to wake that task after it has completed.
    let _h1 = epox::spawn(async {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(nix::sys::timer::Expiration::OneShot(
                std::time::Duration::from_millis(200).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();
    });

    let t = std::rc::Rc::new(std::cell::RefCell::new(epox::Timer::new().unwrap()));
    t.borrow_mut()
        .set(nix::sys::timer::Expiration::OneShot(
            std::time::Duration::from_millis(150).into(),
        ))
        .unwrap();

    let timer = t.clone();

    // this task will wait for the timer in the refcell, but the timeout will cause
    // the whole task to complete before the timer is ready. the waker connected to
    // the timer will still be pointing towards this task even after it has
    // completed.
    let _h2 = epox::spawn(async move {
        #[expect(clippy::await_holding_refcell_ref)]
        epox::time::timeout(std::time::Duration::from_millis(100), async {
            loop {
                timer.borrow_mut().tick().await.unwrap();
            }
        })
        .unwrap()
        .await
        .unwrap_err();
    });

    epox::run().unwrap();

    // make sure first timer hasn't dropped
    assert!(
        t.borrow_mut()
            .tick()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
            .is_ready()
    );
}
