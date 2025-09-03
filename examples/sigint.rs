fn main() {
    epox::spawn(async move {
        asyncmain().await.unwrap();
    });
    epox::run().unwrap();
}

async fn asyncmain() -> Result<(), Box<dyn std::error::Error>> {
    let mut sigset = epox::signal::SigSet::empty();
    sigset.add(epox::signal::Signal::SIGINT);
    let mut signal = epox::AsyncSignal::new(sigset)?;
    let sig = signal.receive().await?;
    println!("done: {sig:#?}");
    println!("2 seconds...");
    let mut timer = epox::Timer::new()?;
    timer.set(epox::timer::Expiration::OneShot(
        std::time::Duration::from_secs(2).into(),
    ))?;
    timer.tick().await?;
    Ok(())
}
