use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use std::io as stdio;
use std::io::{stdout, Write};

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

const SERVER: &str = "127.0.0.1:4321";

#[tokio::main]
async fn main() -> io::Result<()> {

    fmt()
        .compact() // use abbreviated log format
        .with_max_level(Level::DEBUG)
        .with_thread_ids(true) // display thread id where event happens
        .init(); // set as default subscriber

    info!("Client starting, connecting to server {:?}", &SERVER);

    let client = TcpStream::connect(SERVER).await
        .map_err(|e| {error!("Unable to connect to server"); e})?;

    // split tcpstream so we can hand off to r & w tasks
    let (mut client_read, mut client_write) = client.into_split();
    let (local_tx, mut local_rx) = mpsc::channel::<Vec<u8>>(64);

    if let Ok(Some(msg)) = read_user_input("Please input chat name: ") {
        client_write.write_all(&msg).await.expect("Unable to write to server");
    } else {
        error!("Unable to retrieve user chat name");
    }

    /*
    let mut lock = stdout().lock();
    write!(lock, "hello world").unwrap();
    */

    // Spawn client tcp read tokio task, to read back main server msgs
    // TODO: if server is down, need to shut down client e.g. await on handle and make user input loop a task to await
    let _handle = tokio::spawn(async move {
        loop {
            let mut buf = vec![0; 64]; // instead of handling bytes, switch to lines eventually

            // Read bytes from server
            match client_read.read(&mut buf).await {
                Ok(0) => { // 0 signifies remote has closed
                    info!("Server Remote has closed");
                    return;                 }
                Ok(n) => {
                    buf.truncate(n);
                    println!("> {}", std::str::from_utf8(&buf).unwrap());
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

        if let Ok(Some(msg)) = read_user_input("") {
            local_tx.send(msg).await.expect("Unable to tx");
        } else {
            break;
        }
    }

    Ok(())
}

// blocking function to gather user input
fn read_user_input(prompt: &str) -> io::Result<Option<Vec<u8>>> {
//    use std::{thread, time};

    let mut buf = String::new();
//    let t = time::Duration::from_millis(500);
//    thread::sleep(t);
    {
        // lock stdout so the user input is not overwritten with tracing msgs
//        let mut lock = stdout().lock();
//        write!(lock, "{}", prompt).unwrap();
        if prompt != "" {
            print!("{} ", prompt);
            stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
        }
        stdio::stdin().read_line(&mut buf).expect("unable to read command line input");
    }
    let trimmed = buf.trim_end();

    if trimmed == "\\quit" {
        info!("Session terminated by user...");
        return Ok(None);
    }

    Ok(Some(trimmed.as_bytes().to_vec()))
}
