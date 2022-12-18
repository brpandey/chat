use std::io as stdio;
use std::io::{stdout, Write};

use tokio::select;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::io::{self}; //, AsyncWriteExt}; //, AsyncReadExt};

use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

use protocol::{ChatMsg, ChatCodec, Request, Response};

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
    let (client_read, client_write) = client.into_split();
    let (local_tx, mut local_rx) = mpsc::channel::<Request>(64);

    let mut fw = FramedWrite::new(client_write, ChatCodec);

    if let Ok(Some(msg)) = read_sync_user_input(GREETINGS) {
        fw.send(msg).await.expect("Unable to write to server");
    } else {
        error!("Unable to retrieve user chat name");
    }

//    let server_alive_ref1 = Arc::clone(&server_alive);
//    let server_alive_ref2 = Arc::clone(&server_alive);

    let mut fr = FramedRead::new(client_read, ChatCodec);

    // Spawn client tcp read tokio task, to read back main server msgs
    let server_read_handle = tokio::spawn(async move {
        loop {
            debug!("task A: listening to server replies...");

            // Read lines from server
            if let Some(value) = fr.next().await {
                debug!("received server value is {:?}", value);

                match value {
                    Ok(ChatMsg::Response(Response::UserMessage{id, msg})) => {
                        let m = msg.trim_end();
                        println!("> {} {}", id, m);
                    },
                    Ok(ChatMsg::Response(Response::Notification(line))) => {
                        let trimmed = line.trim_end();
                        println!(">>> {}", trimmed);
                    },
                    Ok(_) => unimplemented!(),
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



    // Spawn client tcp write tokio task, to send data to server
    let tcp_write_handle = tokio::spawn(async move {
        loop {
            // Read from channel, data received from command line
            if let Some(msg) = local_rx.recv().await {
                fw.send(msg).await.expect("Unable to write to server");
            }
        }
    });

    // Use current thread to loop and grab data from command line
    let cmd_line_handle = tokio::spawn(async move {
        loop {
            debug!("task C: cmd line input looping");
            if let Ok(Some(msg)) = read_async_user_input().await {
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
fn read_sync_user_input(prompt: &str) -> io::Result<Option<Request>> {
    let mut buf = String::new();

    print!("{} ", prompt);
    stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
    stdio::stdin().read_line(&mut buf).expect("unable to read command line input");

    let name = buf.trim_end().to_string();

    Ok(Some(Request::JoinName(name)))
}

async fn read_async_user_input() -> io::Result<Option<Request>> {
//async fn read_async_user_input() -> io::Result<Option<Vec<u8>>> {
    let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new_with_max_length(256));

    if let Some(Ok(line)) = fr.next().await {
        // handle if any commands present
        match line.as_str() {
            "\\quit" => {
                info!("Session terminated by user...");
                return Ok(Some(Request::Quit));
            },
            "\\users" => {
                return Ok(Some(Request::Users))
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

        let msg: Vec<u8> = total.into_iter().flatten().collect();
        let smsg = String::from_utf8(msg).unwrap();
        return Ok(Some(Request::Message(smsg)))
    }

    Ok(None)
}
