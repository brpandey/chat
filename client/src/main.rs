use std::io as stdio;

use std::io::{stdout, Write};

// use std::sync::atomic::{AtomicBool}, Ordering};
// use std::sync::Arc;

use tokio::select;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::io::{self, AsyncWriteExt}; //, AsyncReadExt};

use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tokio_stream::StreamExt;

use futures::SinkExt;

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

const SERVER: &str = "127.0.0.1:4321";
const GREETINGS: &str = "$ Welcome to chat! \n$ Commands: \\quit, \\users, \\fork chatname\n$ Please input chat name: ";

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact() // use abbreviated log format
        .with_max_level(Level::INFO)
        .with_thread_ids(true) // display thread id where event happens
        .init(); // set as default subscriber

    info!("Client starting, connecting to server {:?}", &SERVER);

    let client = TcpStream::connect(SERVER).await
        .map_err(|e| { error!("Unable to connect to server"); e })?;

//    let server_alive = Arc::new(AtomicBool::new(true));

    // split tcpstream so we can hand off to r & w tasks
    let (client_read, mut client_write) = client.into_split();
    let (local_tx, mut local_rx) = mpsc::channel::<Vec<u8>>(64);

    if let Ok(Some(msg)) = read_sync_user_input(GREETINGS) {
        client_write.write_all(&msg).await.expect("Unable to write to server");
    } else {
        error!("Unable to retrieve user chat name");
    }

//    let server_alive_ref1 = Arc::clone(&server_alive);
//    let server_alive_ref2 = Arc::clone(&server_alive);

    let mut fr = FramedRead::new(client_read, LinesCodec::new());

    // Spawn client tcp read tokio task, to read back main server msgs
      let server_read_handle = tokio::spawn(async move {
        loop {
            debug!("task A: listening to server replies...");

            // Read lines from server
            if let Some(value) = fr.next().await {
                match value {
                    Ok(line) => {
                        let trimmed = line.trim_end();
                        println!("> {}", trimmed);
                    },
                    Err(x) => {
                        debug!("Client Connection closing error: {:?}", x);
                        break;
                    },
                }
            } else {
                info!("Server Remote has closed");
                break;
            }
        }
//        server_alive_ref1.swap(false, Ordering::Relaxed);
    });

    let mut fw = FramedWrite::new(client_write, LinesCodec::new());

    // Spawn client tcp write tokio task, to send data to server
    let tcp_write_handle = tokio::spawn(async move {
        loop {
            // Read from channel, data received from command line
            if let Some(msg) = local_rx.recv().await {
                debug!("Sent msg {:?}, to client write tcp socket", std::str::from_utf8(&msg));
                let m = std::str::from_utf8(&msg).unwrap();
                fw.send(m).await.expect("Unable to write to server");
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
    select! {
        _ = server_read_handle => {
            println!("X");
        }
        _ = cmd_line_handle => {
            println!("Y");
        }
    }

    tcp_write_handle.abort();

    Ok(())
}

// blocking function to gather user input from std::io::stdin
fn read_sync_user_input(prompt: &str) -> io::Result<Option<Vec<u8>>> {
    let mut buf = String::new();

    print!("{} ", prompt);
    stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
    stdio::stdin().read_line(&mut buf).expect("unable to read command line input");

    let trimmed = buf.trim_end();

    Ok(Some(trimmed.as_bytes().to_vec()))
}


async fn read_async_user_input() -> io::Result<Option<Vec<u8>>> {
    let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new_with_max_length(256));

    if let Some(Ok(line)) = fr.next().await {
        // handle if any commands present
        match line.as_str() {
            "\\quit" => {
                info!("Session terminated by user...");
                return Ok(None);
            },
            "\\users" => {
                let res = line.strip_prefix("\\").unwrap().as_bytes().to_vec();
                return Ok(Some(res))
            },
            _ => (),
        }

        let mut total = vec![];
        let mut citer = line.as_bytes().chunks(64);

        while let Some(c) = citer.next() {
            let mut line = c.to_vec();
            line.push(b'\n');
            total.push(line);
        }

        let result: Vec<u8> = total.into_iter().flatten().collect();
        return Ok(Some(result))
    }

    Ok(None)
}
