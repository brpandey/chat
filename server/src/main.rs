use std::collections::{HashMap, HashSet};
use std::sync::Arc;
//use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::select;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener};//, tcp};
use tokio::sync::{mpsc, Mutex, RwLock};

use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_stream::StreamExt;

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

const SERVER: &str = "127.0.0.1:4321";
const ALL_CLIENTS: usize = 0;

const CURRENT_USERS_MSG: &str = "Current users online are: ";
const USER_JOINED: &str = "User {} joined\n";
const USER_LEFT: &str = "User {} has left\n";

// type TcpWrite = tcp::OwnedWriteHalf;


/*
pub struct ClientData {
    id: usize, // Rustinomicon: incrementing a counter can be safely done by multiple threads using a relaxed fetch_add if you're not using the counter to synchronize any other accesses
    addr: SocketAddr;
    name: Vec<u8>,
    tcp_write: TcpWrite,
}

impl ClientData {
    fn new -> Self {
        ClientData {

        }
    }
}
*/

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact()
        .with_max_level(Level::INFO)
        .init();

    info!("Server starting.. {:?}", &SERVER);

    let listener = TcpListener::bind(SERVER).await.expect("Unable to bind to server address");

    // Setup map, allow it to be shared across tasks, which may be passed along threads
    // Setup hashset of names, allow it to be shared across tasks...
    // Setup local msg passing channel
    // Set up unique counter
    // clone tx end of local channel

    let clients = Arc::new(Mutex::new(HashMap::new()));
    let chat_names = Arc::new(RwLock::new(HashSet::new()));

    let (local_tx, mut local_rx) = mpsc::channel::<(usize, Vec<u8>)>(64);
    let counter = Arc::new(AtomicUsize::new(1));
    let task_tx = local_tx.clone(); // clone before spawn

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
                let join_msg: String;

                if let Ok(n) = tcp_read.read(&mut name_buf).await {
                    name_buf.truncate(n);
                    join_msg = USER_JOINED.replace("{}", &std::str::from_utf8(&name_buf).unwrap());
                } else {
                    error!("Unable to retrieve chat user name");
                    return Ok(());
                }

                let client_id = counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client

                // Store (k,v) or (client specific addr, chat name, write tcp socket) in map
                // Need to wrap in Arc/Mutex since it is shared out to 
                // both tokio tasks
                {
                    let mut sc = clients.lock().await;
                    sc.entry(client_id).or_insert((addr.clone(), name_buf.clone(), tcp_write));
                }

                // insert new chat name
                chat_names.write().await.insert(name_buf.clone());

                task_tx.send((ALL_CLIENTS, join_msg.into_bytes())).await.expect("Unable to tx"); // notify others of new join

                let task_tx = task_tx.clone();
                let clients_ref = Arc::clone(&clients);
                let chat_names_ref = Arc::clone(&chat_names);

                // Generate per client msg prefix, e.g. "name: "
                let mut msg_prefix: Vec<u8> = Vec::with_capacity(name_buf.len() + 2);
                msg_prefix.append(&mut name_buf);
                msg_prefix.push(b':');
                msg_prefix.push(b' ');

                let mut fr = FramedRead::new(tcp_read, LinesCodec::new_with_max_length(256));

                // Spawn tokio task to handle server socket reads
                tokio::spawn(async move {
                    loop {
                        // Read lines from server
                        if let Some(value) = fr.next().await {
                            debug!("server waka waka! value is : {:?}", value);

                            match value {
                                Ok(line) if line.len() > 0 => {
                                    debug!("Server received client msg, un-truncated: {:?}", line);

                                    let mut msg: Vec<u8> = Vec::with_capacity(msg_prefix.len() + line.len() + 1);
                                    let mut p = msg_prefix.clone();
                                    let mut line2 = line.into_bytes();
                                    msg.append(&mut p);
                                    msg.append(&mut line2);
                                    msg.push(b'\n');

                                    debug!("Server received client msg {:?}", &msg);
                                    // delegate the broadcast of msg echoing to another block
                                    task_tx.send((client_id, msg)).await.expect("Unable to tx");

                                },
                                Ok(_) => continue,
                                Err(x) => {
                                    debug!("Server Connection closing error: {:?}", x);
                                    break;
                                },
                            }
                        } else {
                            info!("Client has closed");
                            let mut remove_name = vec![];
                            {
                                let mut sc = clients_ref.lock().await;
                                if let Some((addr, name, _w)) = sc.remove(&client_id) {
                                    info!("Remote {:?} has closed connection", &addr);
                                    info!("User {} has left", std::str::from_utf8(&name).unwrap());
                                    remove_name = name;

                                    // broadcast that user has left
                                    let leave_msg = USER_LEFT.replace("{}", &std::str::from_utf8(&remove_name).unwrap());
                                    task_tx.send((client_id, leave_msg.into_bytes())).await.expect("Unable to tx");
                                }
                            }

                            chat_names_ref.write().await.remove(&remove_name);
                            break;
                        }
                    }
                });
            }
            Some((cid, msg)) = local_rx.recv() => {
                // Read data received from server and broadcast to all clients
                debug!("Local channel msg received {:?}", std::str::from_utf8(&msg).unwrap());

                // grab names from chat names which is behind a RWLock
                let mut users_online: Vec<u8> = vec![];

                if cid == ALL_CLIENTS {
                    let mut list: Vec<Vec<u8>>;

                    {
                        let cn = chat_names.read().await;
                        list = Vec::with_capacity(cn.len()*2);
                        for n in cn.iter() {
                            list.push(n.clone());
                            list.push(vec![b' ']);
                        }
                    }

                    let mut joined: Vec<u8> = list.into_iter().flatten().collect();
                    users_online = CURRENT_USERS_MSG.as_bytes().to_vec();
                    users_online.append(&mut joined);
                    users_online.push(b'\n');
                }


                // broadcast to client tcp_write sockets stored in clients which is behind a Mutex
                {
                    let mut sc = clients.lock().await;

                    // broadcast, ws or write_socket needs to mutable
                    for (k, (_addr, _name, ws)) in sc.iter_mut() {
                        // sendto (k) is different than the client id (cid) that sent message
                        // then only send

                        debug!("About to write to server tcp_socket, msg {:?}", std::str::from_utf8(&msg).unwrap());

                        if cid == ALL_CLIENTS {
                            ws.write_all(&users_online).await.expect("Unable to write to tcp socket");
                        }

                        if cid != *k {
                            let err = format!("Unable to write to tcp socket {:?}", k); // debug
                            ws.write_all(&msg).await.expect(&err);
                        }
                    }
                }
            }
        }
    }
}
