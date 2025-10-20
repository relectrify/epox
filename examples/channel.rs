fn main() {
    let (tx, rx) = async_channel::unbounded::<usize>();

    let handle = epox::spawn_checked(async move {
        let begin = std::time::Instant::now();
        println!("waiting for message in async task...");
        let received = rx.recv().await?;
        let took = std::time::Instant::now().duration_since(begin);
        println!("received '{received}' in async task; took {took:?}");
        Ok(())
    });

    std::thread::spawn(move || {
        println!("waiting 2s...");
        std::thread::sleep(std::time::Duration::from_secs(2));
        println!("sending value from other thread");
        tx.send_blocking(420).unwrap();
    });

    epox::run().unwrap();

    handle.result().expect("async task didn't complete");
}
