// all tasks are dropped when the executor is dropped. if the Drop
// implementations of anything in those tasks try to do anything with the
// executor, we will panic: "cannot access a Thread Local Storage value during
// or after destruction: AccessError"

// this tests that the Drop implementation of an epox::Fd doesn't panic after
// the executor has been destroyed.
#[test]
fn drop_task_without_running_executor() -> Result<(), Box<dyn std::error::Error>> {
    let mut timer = epox::Timer::new()?;
    epox::spawn_checked::<_, std::io::Error, _>({
        async move {
            timer.set(epox::timer::Expiration::OneShot(
                std::time::Duration::from_secs(1).into(),
            ))?;
            timer.tick().await?;

            Ok(())
        }
    });

    Ok(())
}
