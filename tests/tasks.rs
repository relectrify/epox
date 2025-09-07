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
