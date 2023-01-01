use std::io as stdio;
use std::io::{stdout, Write};
use std::sync::Arc;

use tokio::select;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, broadcast};

use tokio::io::{self}; //, AsyncWriteExt}; //, AsyncReadExt};

use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait

use tokio::task::JoinSet;

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

use protocol::{ChatMsg, ChatCodec, Request, Response};

use client::peer_client::PeerClient;
use client::peer_server::PeerServerListener;

const GREETINGS: &str = "$ Welcome to chat! \n$ Commands: \\quit, \\users, \\fork chatname\n$ Please input chat name: ";
const SERVER: &str = "127.0.0.1:43210";
const PEER_SERVER: &str = "127.0.0.1:43310";
const PEER_SERVER_PORT: u16 = 43310;
const SHUTDOWN: u8 = 1;
const LINES_MAX_LEN: usize = 256;
const USER_LINES: usize = 64;

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

    info!("Client peer server starting {:?}", &PEER_SERVER);

//    let server_alive = Arc::new(AtomicBool::new(true));
//    let server_alive_ref1 = Arc::clone(&server_alive);
//    let server_alive_ref2 = Arc::clone(&server_alive);

    // split tcpstream so we can hand off to r & w tasks
    let (client_read, client_write) = client.into_split();
    let (local_tx, mut local_rx) = mpsc::channel::<Request>(64);

    let mut fw = FramedWrite::new(client_write, ChatCodec);

    if let Ok(Some(msg)) = read_sync_user_input(GREETINGS) {
        fw.send(msg).await.expect("Unable to write to server");
    } else {
        error!("Unable to retrieve user chat name");
    }

    let mut client_name = String::new();
    let mut fr = FramedRead::new(client_read, ChatCodec);

    if let Some(Ok(ChatMsg::Server(Response::JoinNameAck(name_bytes)))) = fr.next().await {
        println!(">>> Registered as name: {}", std::str::from_utf8(&name_bytes).unwrap());
        client_name = String::from_utf8(name_bytes).unwrap_or_default();
    }

    // start up peer server for clients that connect to this node for
    // peer to peer chat.
    PeerServerListener::spawn_accept(PEER_SERVER.to_owned(), client_name.clone());

    let peer_clients = Arc::new(Mutex::new(JoinSet::new()));
    let set = peer_clients.clone(); 

    let (shutdown_tx1, mut shutdown_rx1) = broadcast::channel(16);
    let mut shutdown_rx2 = shutdown_tx1.subscribe();
    let mut shutdown_rx3 = shutdown_tx1.subscribe();
    let mut shutdown_rx4 = shutdown_tx1.subscribe();

    let shutdown_tx2 = shutdown_tx1.clone();
    let shutdown_tx3 = shutdown_tx1.clone();

    // Spawn client tcp read tokio task, to read back main server msgs
    let _server_read_handle = tokio::spawn(async move {
        loop {
            debug!("task A: listening to server replies...");

            select! {
                // Read lines from server
                Some(value) = fr.next() => {
                    debug!("received server value is {:?}", value);

                    match value {
                        Ok(ChatMsg::Server(Response::UserMessage{id, msg})) => {
                            println!("> {} {}", id, std::str::from_utf8(&msg).unwrap());
                        },
                        Ok(ChatMsg::Server(Response::Notification(line))) => {
                            println!(">>> {}", std::str::from_utf8(&line).unwrap());
                        },
                        Ok(ChatMsg::Server(Response::ForkPeerAckA{id, name, mut addr})) => {
                            println!(">>> About to fork private session with {} {}", id, std::str::from_utf8(&name).unwrap());

                            // spawn tokio task to send client requests to peer server address
                            // responding to server requests will take a lower priority
                            // unless the user explicitly switches over to communicating with the server from
                            // peer to peer mode

                            // drop current port of addr and add PEER_SERVER_PORT to addr
                            addr.set_port(PEER_SERVER_PORT);
                            let addr_string = format!("{}", &addr);
                            set.lock().await.spawn(PeerClient::nospawn_a(addr_string, client_name.clone()));

                            // should probably set some cond var that blocks other threads from reading from stdin
                            // until user explicitly switches to client -> server mode
                        },
                        Ok(ChatMsg::Server(Response::PeerUnavailable(name))) => {
                            println!(">>> Unable to fork into private session as peer {} unavailable",
                                     std::str::from_utf8(&name).unwrap());
                        },
                        Ok(_) => unimplemented!(),
                        Err(x) => {
                            debug!("Client Connection closing error: {:?}", x);
                            break;
                        }
                    }
                } // exit task if shutdown received
   /*             _ = shutdown_rx1.recv() => {
                    info!("server_read_handle received shutdown, returning!");
                    return;
                }
   */             else => {
                    info!("Server Remote has closed");
                    info!("sending shutdown msg A");
                    shutdown_tx1.send(SHUTDOWN).expect("Unable to send shutdown");
                    break;
                }
            };
        //        server_alive_ref1.swap(false, Ordering::Relaxed);
        }
    });

    // Spawn client tcp write tokio task, to send data to server
    let _tcp_write_handle = tokio::spawn(async move {
        loop {
            select! {
                // Read from channel, data received from command line
                Some(msg) = local_rx.recv() => {
                    fw.send(msg).await.expect("Unable to write to server");
                }
                _ = shutdown_rx2.recv() => {
                    info!("tcp_write_handle received shutdown, returning!");
                    return;  // exit task if shutdown received
                }
 /*               else => {
                    shutdown_tx2.send(SHUTDOWN).expect("Unable to send shutdown");
                    info!("sending shutdown msg B");
                    break;
                }
                */
            }
        }
    });

    // Use current thread to loop and grab data from command line
    let _cmd_line_handle = tokio::spawn(async move {
        loop {
            debug!("task C: cmd line input looping");
            select! {
                Ok(Some(msg)) = read_async_user_input() => {
                    local_tx.send(msg).await.expect("xxx Unable to tx");
                }
                _ = shutdown_rx3.recv() => {
                    info!("cmd_line_handle received shutdown, returning!");
                    break; // exit task if shutdown received
                }
                else => {
                    shutdown_tx3.send(SHUTDOWN).expect("Unable to send shutdown");
                    info!("sending shutdown msg C");
                    break;
                }
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

    loop {
        select! {
            _ = shutdown_rx4.recv() => { // exit task if shutdown received
                info!("received final shutdown");
                break;
            }
        };
    }

    /*

    select! {
        _ = server_read_handle => { // fires as soon as future completes
      //      tcp_write_handle.abort();
            println!("X");
        }
        _ = cmd_line_handle => {
            println!("Y");
        }
    }

    */

    // consume arc and lock
    let mut clients = Arc::try_unwrap(peer_clients).unwrap().into_inner();

    info!("clients are {:?}", clients);

    while let Some(res) = clients.join_next().await {
        info!("peer client completed {:?}", res);
    }

    info!("waka waka!");

    Ok(())
}

// blocking function to gather user input from std::io::stdin
fn read_sync_user_input(prompt: &str) -> io::Result<Option<Request>> {
    let mut buf = String::new();

    print!("{} ", prompt);
    stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
    stdio::stdin().read_line(&mut buf).expect("unable to read command line input");

    let name = buf.trim_end().as_bytes().to_owned();

    Ok(Some(Request::JoinName(name)))
}

async fn read_async_user_input() -> io::Result<Option<Request>> {
    let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new_with_max_length(LINES_MAX_LEN));

    if let Some(Ok(line)) = fr.next().await {
        // handle user inputted commands
        match line.as_str() {
            "\\quit" => {
                info!("Session terminated by user...");
                return Ok(Some(Request::Quit));
            },
            "\\users" => {
                return Ok(Some(Request::Users))
            },
            value if value.starts_with("\\fork") => {
                if let Some(name) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let name: Vec<u8> = name.as_bytes().to_owned();
                    info!("Attempting to fork a session with {}", std::str::from_utf8(&name).unwrap());
                    // todo -- local validation if name is present
                    return Ok(Some(Request::ForkPeer{name}));
                }
            },
            _ => (),
        }

        // if no commands, split up user input
        let mut total = vec![];
        let mut citer = line.as_bytes().chunks(USER_LINES);

        while let Some(c) = citer.next() {
            let mut line = c.to_vec();
            line.push(b'\n');
            total.push(line);
        }

        let msg: Vec<u8> = total.into_iter().flatten().collect();
        return Ok(Some(Request::Message(msg)))
    }

    Ok(None)
}
