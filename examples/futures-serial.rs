use clap::Parser;
use futures_lite::{AsyncReadExt, AsyncWriteExt};

#[derive(Parser)]
struct Args {
    #[clap(default_value = "/dev/ttyUSB0")]
    port: String,
    #[clap(default_value_t = 115_200)]
    baud_rate: u32,
}

// this makes the most sense with a loopback serial port
fn main() {
    let args = Args::parse();
    let rx = serialport::new(args.port, args.baud_rate)
        .open_native()
        .unwrap();

    let tx = rx.try_clone_native().unwrap();

    let rx = epox::futures::AsyncReadFd::new(rx).unwrap();
    let tx = epox::futures::AsyncWriteFd::new(tx).unwrap();

    epox::spawn(async move {
        rx_task(rx).await.unwrap();
    });
    epox::spawn(async move {
        tx_task(tx).await.unwrap();
    });
    epox::run().unwrap();
}

async fn rx_task<F: futures_io::AsyncRead + Unpin>(
    mut rx: F,
) -> Result<(), Box<dyn core::error::Error>> {
    let mut timer = epox::Timer::new()?;
    timer.set(epox::timer::Expiration::OneShot(
        std::time::Duration::from_secs(1).into(),
    ))?;
    timer.tick().await?;
    println!("waiting for serial data...");
    let mut buf = [0; 32];
    let len = loop {
        println!("about to read");
        let len = rx.read(&mut buf).await?;
        if len > 0 {
            break len;
        }
        println!(":(");
    };
    println!("read {:#?}", String::from_utf8_lossy(&buf[0..len]));
    Ok(())
}

async fn tx_task<F: futures_io::AsyncWrite + Unpin>(
    mut tx: F,
) -> Result<(), Box<dyn core::error::Error>> {
    const TEST_STR: &str = "test string from tx task";
    tx.write_all(TEST_STR.as_bytes()).await?;
    println!("finished tx");
    Ok(())
}
