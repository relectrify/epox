fn main() {
    let mut timer = epox::Timer::new().unwrap();
    timer
        .set_time(core::time::Duration::from_millis(250), None)
        .unwrap();
    epox::spawn(async move {
        println!("waiting for timer");
        assert!(timer.await.unwrap() == 1);
        println!("timer expired");
    });
    epox::run().unwrap();
}
