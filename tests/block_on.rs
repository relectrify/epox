/*
 * Make sure that the future passed to block_on can run to completion.
 */

#[test]
fn block_on() {
    epox::block_on(async {
        epox::time::sleep(std::time::Duration::from_millis(100))
            .await
            .unwrap();
    });
}

#[test]
fn block_on_thread() {
    epox::block_on(async {
        epox::thread::spawn(|| std::thread::sleep(std::time::Duration::from_millis(100)))
            .join()
            .await
            .unwrap();
    });
}
