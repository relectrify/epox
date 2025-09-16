#[test]
fn yield_to_executor() {
    let io = epox::spawn(io_task());
    let blocking = epox::spawn(blocking_task());
    epox::run().unwrap();

    let (io_steps, io_duration) = io.result().unwrap().unwrap();
    let (blocking_steps, blocking_duration) = blocking.result().unwrap().unwrap();

    assert_eq!(io_steps, IO_TASK_STEPS);
    assert_eq!(blocking_steps, BLOCKING_TASK_STEPS);

    // each task should have taken TOTAL_DURATION +/- 10%
    assert!(io_duration.abs_diff(TOTAL_DURATION) < TOTAL_DURATION / 10);
    assert!(blocking_duration.abs_diff(TOTAL_DURATION) < TOTAL_DURATION / 10);
}

const TOTAL_DURATION: std::time::Duration = std::time::Duration::from_secs(1);
const IO_TASK_STEPS: u32 = 10;
const BLOCKING_TASK_STEPS: u32 = IO_TASK_STEPS * 3;

async fn io_task() -> Result<(u32, std::time::Duration), Box<dyn std::error::Error>> {
    let t = std::time::Instant::now();
    let mut steps_taken = 0;
    let mut timer = epox::Timer::new()?;
    timer.set(epox::timer::Expiration::Interval(
        (TOTAL_DURATION / IO_TASK_STEPS).into(),
    ))?;

    for _ in 0..(IO_TASK_STEPS) {
        timer.tick().await?;
        steps_taken += 1;
    }
    let took = std::time::Instant::now().duration_since(t);

    Ok((steps_taken, took))
}

async fn blocking_task() -> Result<(u32, std::time::Duration), Box<dyn std::error::Error>> {
    let t = std::time::Instant::now();
    let mut steps_taken = 0;

    for _ in 0..BLOCKING_TASK_STEPS {
        std::thread::sleep(TOTAL_DURATION / BLOCKING_TASK_STEPS);
        epox::yield_now().await;
        steps_taken += 1;
    }
    let took = std::time::Instant::now().duration_since(t);

    Ok((steps_taken, took))
}
