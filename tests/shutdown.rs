const SHUTDOWN_AFTER_MILLIS: u64 = 100;

const SHUTDOWN_AFTER: std::time::Duration = std::time::Duration::from_millis(100);
const WILL_COMPLETE: std::time::Duration =
    std::time::Duration::from_millis(SHUTDOWN_AFTER_MILLIS / 2);
const WILL_NOT_COMPLETE: std::time::Duration =
    std::time::Duration::from_millis(SHUTDOWN_AFTER_MILLIS * 2);

#[test]
fn shutdown() {
    let calls_shutdown = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(SHUTDOWN_AFTER.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; shutdown now");

        epox::shutdown().await;
    });
    let will_complete = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(WILL_COMPLETE.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; will_complete complete");

        "return value"
    });
    let will_not_complete = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(WILL_NOT_COMPLETE.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; will_not_complete complete");

        "return value"
    });

    epox::run().unwrap();

    assert!(will_complete.result().is_some());
    assert!(will_not_complete.result().is_none());
    assert!(calls_shutdown.result().is_none());
}

#[test]
fn shutdown_executor_unchecked() {
    let calls_shutdown = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(SHUTDOWN_AFTER.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; shutdown now");

        unsafe { epox::executor::shutdown_executor_unchecked() };
        "return value"
    });
    let will_complete = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(WILL_COMPLETE.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; will_complete complete");

        "return value"
    });
    let will_not_complete = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(WILL_NOT_COMPLETE.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; will_not_complete complete");

        "return value"
    });

    epox::run().unwrap();

    assert!(will_complete.result().is_some());
    assert!(will_not_complete.result().is_none());
    // shutdown_executor_unchecked lets calls_shutdown return a value
    assert!(calls_shutdown.result().is_some());
}

#[test]
fn spawn_checked() {
    let will_complete = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(WILL_COMPLETE.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; will_complete complete");

        "return value"
    });
    // will loop 3x before returning an error, and should never return Ok
    let will_error = epox::spawn_checked::<(), Box<dyn std::error::Error>, _>(async move {
        let mut num = 0;
        loop {
            let mut timer = epox::Timer::new().unwrap();
            timer
                .set(epox::timer::Expiration::OneShot(
                    (SHUTDOWN_AFTER / 3).into(),
                ))
                .unwrap();
            timer.tick().await.unwrap();

            (num < 3).then_some(()).ok_or("error")?;

            num += 1;
        }
    });

    let will_not_complete = epox::spawn(async move {
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(WILL_NOT_COMPLETE.into()))
            .unwrap();
        timer.tick().await.unwrap();
        println!("timer finished; will_not_complete complete");

        "return value"
    });

    epox::run().unwrap();

    assert!(will_error.result().is_some_and(|r| r.is_err()));

    assert!(will_complete.result().is_some());
    assert!(will_not_complete.result().is_none());
}
