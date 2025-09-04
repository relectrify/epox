fn main() {
    epox::spawn(async {
        // normal io-bound async task
        io_task().await.unwrap();
    });
    epox::spawn(async {
        // simulate doing a lot of work on the cpu, manually letting the executor run
        // other tasks periodically
        blocking_task().await.unwrap();
    });
    epox::run().unwrap();
}

const TOTAL_DURATION: std::time::Duration = std::time::Duration::from_secs(3);
const IO_TASK_STEPS: u32 = 10;
const BLOCKING_TASK_STEPS: u32 = IO_TASK_STEPS * 3;

async fn io_task() -> Result<(), Box<dyn std::error::Error>> {
    let t = std::time::Instant::now();
    let mut timer = epox::Timer::new()?;
    timer.set(epox::timer::Expiration::Interval(
        (TOTAL_DURATION / IO_TASK_STEPS).into(),
    ))?;
    println!("starting io task now");
    for i in 0..(IO_TASK_STEPS) {
        println!("timer tick {i}...");
        timer.tick().await?;
    }
    let took = std::time::Instant::now().duration_since(t);
    println!("finished io task - took {took:?}");
    Ok(())
}

async fn blocking_task() -> Result<(), Box<dyn std::error::Error>> {
    let t = std::time::Instant::now();
    println!("starting blocking task now");
    for i in 0..BLOCKING_TASK_STEPS {
        println!("blocking tick {i}");

        // pretend to do some computationally intensive work
        std::thread::sleep(TOTAL_DURATION / BLOCKING_TASK_STEPS);

        epox::yield_now().await;
    }
    let took = std::time::Instant::now().duration_since(t);
    println!("finished blocking task - took {took:?}");
    Ok(())
}
