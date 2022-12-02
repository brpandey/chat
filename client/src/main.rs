use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use std::io as stdio;

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

const SERVER: &str = "127.0.0.1:4321";

#[tokio::main]
async fn main() -> io::Result<()> {

    fmt()
        .compact() // use abbreviated log format
        .with_max_level(Level::INFO)
        .with_thread_ids(true) // display thread id where event happens
        .init(); // set as default subscriber

    info!("Client starting, connecting to server {:?}", &SERVER);

    let client = TcpStream::connect(SERVER).await
        .map_err(|e| {error!("Unable to connect to server"); e})?;

    // split tcpstream so we can hand off to r & w tasks
    let (mut client_read, mut client_write) = client.into_split();
    let (local_tx, mut local_rx) = mpsc::channel::<Vec<u8>>(64);

    // Spawn client tcp read tokio task, to read back main server msgs
    tokio::spawn(async move {
        loop {
            let mut buf = vec![0; 64]; // instead of handling bytes, switch to lines eventually

            // Read bytes from server
            match client_read.read(&mut buf).await {
                Ok(0) => { // 0 signifies remote has closed
                    info!("Server Remote has closed");
                    return;                 }
                Ok(_n) => {
                    buf = buf.into_iter().filter(|&x| if x > 0 { true } else { false }).collect();
                    info!("Server: {:?}", std::str::from_utf8(&buf).unwrap());
                },
                Err(_) => {
                    debug!("Client Connection closing");
                    break;
                }
            }
        }
    });

    // Spawn client tcp write tokio task, to send data to server
    tokio::spawn(async move {
        loop {
            // Read from channel, data received from command line
            if let Some(msg) = local_rx.recv().await {
                client_write.write_all(&msg).await.expect("Unable to write to server");
            }
        }
    });

    // Use current thread to loop and grab data from command line
    loop {
        let mut buf = String::new();
        stdio::stdin().read_line(&mut buf).expect("unable to read command line input");
        let trimmed = buf.trim_end();

        if trimmed == "\\quit" { break; }

        local_tx.send(trimmed.as_bytes().to_vec()).await.expect("Unable to tx");
    }

    info!("Session terminated by user...");
    Ok(())
}
