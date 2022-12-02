use std::collections::HashMap;
use std::sync::Arc;

use tokio::select;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::{info, debug, Level};

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
                let (mut read, write) = tcp_socket.into_split();

                info!("Server received new client connection {:?}", &addr);

                // Store (k,v) or (client specific addr, write tcp socket) in map
                // Need to wrap in Arc/Mutex since it is shared out to 
                // both tokio tasks
                {
                    let mut sc = clients.lock().await;
                    sc.entry(addr).or_insert(write);
                }

                let (task_tx, c_addr) = (local_tx.clone(), addr.clone()); // clone before spawn
                let clients_ref = Arc::clone(&clients);

                // Spawn tokio task to handle server socket reads
                tokio::spawn(async move {
                    loop {
                        let mut buf = vec![0; 64];

                        // Read msg from server socket connection
                        // Fire off to channel, keeping read loop tight
                        match read.read(&mut buf).await {
                            Ok(0) => { // 0 signifies remote has closed
                                info!("Remote {:?} has closed connection", &c_addr);
                                {
                                    let mut sc = clients_ref.lock().await;
                                    sc.remove(&c_addr);
                                }
                                // broadcast that client_addr has left
                                break; 
                            },
                            Ok(n) => {
                                buf = buf.into_iter().filter(|&x| if x > 0 { true } else { false }).collect();
                                debug!("Server received client msg {:?}", &buf);
                                // delegate the broadcast of msg echoing to another block
                                task_tx.send(buf[..n].to_vec()).await.expect("Unable to tx");
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

                    // broadcast, v or write_socket needs to mutable
                    for (_k, v) in sc.iter_mut() {
                        v.write_all(&msg).await.expect("Unable to write to tcp socket");
                    }
                }
            }
        }
    }
}
