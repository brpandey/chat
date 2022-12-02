use std::collections::HashMap;
use std::sync::Arc;

use tokio::select;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

const SERVER: &str = "127.0.0.1:4321";

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact()
        .with_max_level(Level::INFO)
        .init();

    info!("Server starting.. {:?}", &SERVER);

    let listener = TcpListener::bind(SERVER).await.expect("Unable to bind to server address");
    // Allow map to be shared across tasks, which may be passed along threads
    let clients = Arc::new(Mutex::new(HashMap::new())); 
    let (local_tx, mut local_rx) = mpsc::channel::<Vec<u8>>(64);


    // Loop to handle either
    // 1) async listening for new clients, or
    // 2) async channel read new data
    loop {
        select! {
            Ok((tcp_socket, addr)) = listener.accept() => {
                let (mut tcp_read, tcp_write) = tcp_socket.into_split();

                info!("Server received new client connection {:?}", &addr);

                // Wait for chat name msg
                let mut name_buf: Vec<u8> = vec![0; 64];

                if let Ok(n) = tcp_read.read(&mut name_buf).await {
                    name_buf.truncate(n);
                    info!("User {:?} joined", &std::str::from_utf8(&name_buf).unwrap());
                } else {
                    error!("Unable to retrieve chat user name");
                    return Ok(());
                }

                // Store (k,v) or (client specific addr, write tcp socket) in map
                // Need to wrap in Arc/Mutex since it is shared out to 
                // both tokio tasks
                {
                    let mut sc = clients.lock().await;
                    sc.entry(addr).or_insert((name_buf.clone(), tcp_write));
                }

                let (task_tx, c_addr) = (local_tx.clone(), addr.clone()); // clone before spawn
                let clients_ref = Arc::clone(&clients);

                let mut msg_prefix: Vec<u8> = Vec::with_capacity(name_buf.len() + 2);
                msg_prefix.append(&mut name_buf);
                msg_prefix.push(b':');
                msg_prefix.push(b' ');

                // Spawn tokio task to handle server socket reads
                tokio::spawn(async move {
                    loop {
                        let mut buf = vec![0; 64];

                        // Read msg from server socket connection
                        // Fire off to channel, keeping read loop tight
                        match tcp_read.read(&mut buf).await {
                            Ok(0) => { // 0 signifies remote has closed
                                info!("Remote {:?} has closed connection", &c_addr);
                                let mut user = vec![];
                                {
                                    let mut sc = clients_ref.lock().await;
                                    if let Some((name, _w)) = sc.remove(&c_addr) {
                                        user = name;
                                    }
                                }
                                info!("User {:?} has left the chat", std::str::from_utf8(&user).unwrap());
                                // broadcast that user has left (todo)
                                break;
                            },
                            Ok(n) => {
                                buf.truncate(n);
                                let mut msg: Vec<u8> = Vec::with_capacity(msg_prefix.len() + buf.len());
                                let mut p = msg_prefix.clone();
                                msg.append(&mut p);
                                msg.append(&mut buf);

                                debug!("Server received client msg {:?}", std::str::from_utf8(&msg).unwrap());
                                // delegate the broadcast of msg echoing to another block
                                task_tx.send(msg).await.expect("Unable to tx");
                            },
                            Err(_) => {
                                debug!("Server connection closing");
                                return;
                            }
                        }
                    }
                });
            }
            Some(msg) = local_rx.recv() => {
                // Read data received from server and broadcast to all clients
                debug!("Local channel msg received {:?}", &msg);

                {
                    let mut sc = clients.lock().await;

                    // broadcast, ws or write_socket needs to mutable
                    for (k, (_name, ws)) in sc.iter_mut() {
                        let err = format!("Unable to write to tcp socket {:?}", k); // debug
                        ws.write_all(&msg).await.expect(&err);
                    }
                }
            }
        }
    }
}
