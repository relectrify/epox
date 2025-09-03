fn main() {
    println!("spawning t1");
    let t1 = epox::spawn(async {
        println!("t1 running");
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(100).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();
        println!("t1 done");
        20
    });

    println!("spawning t2");
    let t2 = epox::spawn(async {
        println!("t2 running");
        let mut timer = epox::Timer::new().unwrap();
        timer
            .set(epox::timer::Expiration::OneShot(
                core::time::Duration::from_millis(200).into(),
            ))
            .unwrap();
        timer.tick().await.unwrap();
        println!("t2 done");
        22
    });

    println!("spawning t3");
    let t3 = epox::spawn(async {
        println!("t3 running");
        let result = t1.await + t2.await;
        println!("t3 done");
        result
    });

    println!("running executor");
    epox::run().unwrap();

    println!("t3 returned {}", t3.result().unwrap());
}
