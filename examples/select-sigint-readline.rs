use futures::{AsyncBufReadExt, FutureExt};

fn main() {
    epox::spawn(async move {
        asyncmain().await.unwrap();
    });
    epox::run().unwrap();
}

async fn asyncmain() -> Result<(), Box<dyn std::error::Error>> {
    let mut sigset = epox::signal::SigSet::empty();
    sigset.add(epox::signal::Signal::SIGINT);
    let mut signal = epox::SignalHandler::new(sigset)?;

    futures::select! {
        _ = respond_to_lines().fuse() => {
            println!("quit command");
        },
        _ = signal.received().fuse() => {
            println!("sigint");
        }
    };
    Ok(())
}

async fn respond_to_lines() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdin = StdinLineRead::new()?;
    loop {
        let line = stdin.next().await?;
        if line == "quit" {
            break Ok(());
        }
        println!("got '{line}'");
    }
}

struct StdinLineRead {
    fd: futures::io::BufReader<epox::ReadableFd<std::io::Stdin>>,
}

impl StdinLineRead {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let fd = futures::io::BufReader::new(epox::ReadableFd::new(std::io::stdin())?);
        Ok(Self { fd })
    }

    async fn next(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        let mut s = String::new();
        self.fd.read_line(&mut s).await?;
        Ok(s.trim().to_string())
    }
}
