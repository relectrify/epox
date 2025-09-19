const SLEEP_DURATION: std::time::Duration = std::time::Duration::from_secs(1);

#[test]
fn channel() {
    let (tx, rx) = async_channel::unbounded::<usize>();

    let handle = epox::spawn(async move {
        match epox::time::timeout(SLEEP_DURATION * 2, async move { rx.recv().await })
            .unwrap()
            .await
        {
            Ok(Ok(v)) => {
                // received value
                println!("got value {v}");
                true
            }
            Ok(Err(e)) => {
                // recv returned error
                println!("recv returned error: {e:?}");
                false
            }
            Err(()) => {
                // timed out
                false
            }
        }
    });

    std::thread::spawn(move || {
        std::thread::sleep(SLEEP_DURATION);
        tx.send_blocking(420).unwrap();
    });

    epox::run().unwrap();

    let received_value = handle.result().expect("async task didn't complete");
    assert!(received_value);
}
