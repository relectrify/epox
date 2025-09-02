fn main() {
    let mut timer = epox::Timer::new().unwrap();
    timer
        .set_time(
            core::time::Duration::from_millis(100),
            Some(core::time::Duration::from_millis(100)),
        )
        .unwrap();
    epox::spawn(async move {
        for n in 0..3 {
            println!("waiting for timer {n}");
            assert!(timer.tick().await.unwrap() == 1);
        }
    });
    epox::run().unwrap();
}
