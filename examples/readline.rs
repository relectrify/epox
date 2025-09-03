use futures_lite::AsyncBufReadExt;

fn main() {
    epox::spawn(async move {
        asyncmain().await.unwrap();
    });
    epox::run().unwrap();
}

async fn asyncmain() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let stdin_fd = epox::futures::AsyncReadFd::new(stdin)?;
    let mut stdin_reader = futures_lite::io::BufReader::new(stdin_fd);
    loop {
        let mut line = String::new();
        stdin_reader.read_line(&mut line).await?;
        let line = line.trim();
        if line == "quit" {
            break;
        }
        println!("got '{line}'");
    }
    Ok(())
}
