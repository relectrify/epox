use epox::Timer;

const TIMEOUT_DURATION: std::time::Duration = std::time::Duration::from_secs(1);

#[test]
fn timeout() {
    let handle = epox::spawn_checked(async {
        match epox::time::timeout(TIMEOUT_DURATION, takes_one_minute())?.await {
            Ok(_) => Ok(false),
            Err(()) => Ok(true),
        }
    });

    let begin = std::time::Instant::now();
    epox::run().unwrap();
    let took = std::time::Instant::now().duration_since(begin);
    let handle_timed_out = handle.result().expect("task should have finished");
    assert!(handle_timed_out);
    // should be within 5% of TIMEOUT_DURATION
    assert!(took.abs_diff(TIMEOUT_DURATION) < (TIMEOUT_DURATION / 20));
}

async fn takes_one_minute() -> std::io::Result<()> {
    let mut timer = Timer::new()?;
    timer.set(nix::sys::timer::Expiration::OneShot(
        std::time::Duration::from_secs(60 * 2).into(),
    ))?;
    timer.tick().await?;
    Ok(())
}

#[test]
fn sleep() {
    epox::spawn(async {
        epox::time::sleep(TIMEOUT_DURATION).await.unwrap();
    });

    let begin = std::time::Instant::now();
    epox::run().unwrap();
    let took = std::time::Instant::now().duration_since(begin); // should be within 5% of TIMEOUT_DURATION
    assert!(took.abs_diff(TIMEOUT_DURATION) < (TIMEOUT_DURATION / 20));
}
