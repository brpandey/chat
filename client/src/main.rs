use std::io as stdio;

use std::io::{stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio_util::codec::{FramedRead, LinesCodec};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;

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
        .map_err(|e| { error!("Unable to connect to server"); e })?;

    let server_alive = Arc::new(AtomicBool::new(true));

    // split tcpstream so we can hand off to r & w tasks
    let (mut client_read, mut client_write) = client.into_split();
    let (local_tx, mut local_rx) = mpsc::channel::<Vec<u8>>(64);

    if let Ok(Some(msg)) = read_sync_user_input("$ Welcome to chat! \n$ Commands (\\quit, \\private user_name). \n$ Please input chat name: ") {
        client_write.write_all(&msg).await.expect("Unable to write to server");
    } else {
        error!("Unable to retrieve user chat name");
    }

//    let server_alive_ref1 = Arc::clone(&server_alive);
//    let server_alive_ref2 = Arc::clone(&server_alive);

    // Spawn client tcp read tokio task, to read back main server msgs
    // TODO: if server is down, need to shut down client e.g. await on handle and make user input loop a task to await
    let server_read_handle = tokio::spawn(async move {
        loop {
            debug!("task A: listening to server replies...");

            let mut buf = vec![0; 64]; // instead of handling bytes, switch to lines eventually

            // Read bytes from server
            match client_read.read(&mut buf).await {
                Ok(0) => { // 0 signifies remote has closed
                    info!("Server Remote has closed");
                    break;
                },
                Ok(n) => {
                    buf.truncate(n); // TODO need to change to LinesCodec so we don't print multiple lines
                    println!("> {}", std::str::from_utf8(&buf).unwrap());
                },
                Err(_) => {
                    debug!("Client Connection closing");
                    break;
                }
            }
        }

        server_alive_ref1.swap(false, Ordering::Relaxed);
    });

    // Spawn client tcp write tokio task, to send data to server
    let tcp_write_handle = tokio::spawn(async move {
        loop {
            // Read from channel, data received from command line
            if let Some(msg) = local_rx.recv().await {
                debug!("Sent msg {:?}, to client write tcp socket", &msg);
                client_write.write_all(&msg).await.expect("Unable to write to server");
            }
        }
    });

    // Use current thread to loop and grab data from command line
    let cmd_line_handle = tokio::spawn(async move {
        loop {
            debug!("task C: cmd line input looping");
            if let Ok(Some(msg)) = read_async_user_input().await {
                debug!("about to send cmd line msg {:?} to local channel", &msg);
                local_tx.send(msg).await.expect("xxx Unable to tx");
            } else {
                break;
            }

            /*
            // If main server is dead, stop accepting cmd line input
            if !server_alive_ref2.load(Ordering::Relaxed) {
                debug!("Server not alive, hence exiting cmd line processing");
                break;
            }
            */

        }
    });

    // Note: use try_join() and all the tasks should be async functions
    tokio::select! {
        _ = server_read_handle => {
            println!("X");
        }
        _ = cmd_line_handle => {
            println!("Y");
        }
//        _ = tcp_write_handle => {
//            println!("Z");
//        }
    }

    tcp_write_handle.abort();

    //    cmd_line_handle.await.unwrap();


    Ok(())
}

// blocking function to gather user input from std::io::stdin
fn read_sync_user_input(prompt: &str) -> io::Result<Option<Vec<u8>>> {
    let mut buf = String::new();
    {
        if prompt != "" {
            print!("{} ", prompt);
            stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
        }
        stdio::stdin().read_line(&mut buf).expect("unable to read command line input");
    }

    let trimmed = buf.trim_end();

    Ok(Some(trimmed.as_bytes().to_vec()))
}


async fn read_async_user_input() -> io::Result<Option<Vec<u8>>> {
    let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new());

    if let Some(Ok(line)) = fr.next().await {
        let trimmed = line.trim_end();

        if trimmed == "\\quit" {
            info!("Session terminated by user...");
            return Ok(None);
        }
        return Ok(Some(trimmed.as_bytes().to_vec()))
    }

    Ok(None)

}
