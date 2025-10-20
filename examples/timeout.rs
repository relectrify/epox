use futures_lite::AsyncBufReadExt;

fn main() {
    let handle = epox::spawn_checked(async {
        let mut stdin_reader =
            futures_lite::io::BufReader::new(epox::futures::AsyncReadFd::new(std::io::stdin())?);
        let mut line = String::new();
        match epox::time::timeout(std::time::Duration::from_secs(2), async move {
            stdin_reader.read_line(&mut line).await?;
            let line = line.trim();
            println!("read line: {line}");
            Ok::<(), std::io::Error>(())
        })?
        .await
        {
            Ok(ret) => {
                ret?;
            }
            Err(()) => println!("timed out waiting for line"),
        }
        Ok(())
    });

    epox::run().unwrap();
    handle.result().unwrap();
}
