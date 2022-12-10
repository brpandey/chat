use std::collections::HashMap;
use std::time::Duration;
use std::sync::Arc;
//use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::select;
use tokio::io::{self, AsyncReadExt};
use tokio::net::TcpListener; //, tcp};
use tokio::sync::{mpsc, Mutex};

use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_stream::StreamExt;

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

use server::server::Registry;
use server::delivery::Delivery;
use server::names::NamesShared;

const SERVER: &str = "127.0.0.1:4321";

const COUNTER_SEED: usize = 1;
const LINES_MAX_LEN: usize = 256;
const BOUNDED_CHANNEL_SIZE: usize = 64;

const USER_JOINED: &str = "User {} joined\n";
const USER_JOINED_ACK: &str = "You have successfully joined as {} \n";
const USER_LEFT: &str = "User {} has left\n";

#[derive(Debug)]
pub enum MsgType {
    Joined(usize, Vec<u8>),
    JoinedAck(usize, Vec<u8>),
    Message(usize, Vec<u8>),
    MessageSingle(usize, Vec<u8>),
    Exited(Vec<u8>),
    Users(usize),
}

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact()
        .with_max_level(Level::INFO)
        .init();

    info!("Server starting.. {:?}", &SERVER);

    let listener = TcpListener::bind(SERVER).await.expect("Unable to bind to server address");

    // Setup registry map, allow it to be shared across tasks, which may be passed along threads
    // share registry with Delivery
    let clients: Registry = Arc::new(Mutex::new(HashMap::new()));
    let mut outgoing = Delivery::new(&clients);

    // Setup two shared copies of a set of unique chat names
    // which are shared across tokio tasks
    let chat_names1: NamesShared = NamesShared::new();
    let chat_names2: NamesShared = chat_names1.clone();

    // Setup local msg passing channel
    // clone tx end of local channel

    let (local_tx, mut local_rx) = mpsc::channel::<MsgType>(BOUNDED_CHANNEL_SIZE);
    let task_tx1 = local_tx.clone(); // clone before spawn
    let task_tx2 = local_tx.clone(); // clone before spawn

    // Set up unique counter
    let counter = Arc::new(AtomicUsize::new(COUNTER_SEED));

    // Loop to handle either
    // 1) async listening for new clients, or
    // 2) async channel read new data
    loop {
        select! {
            Ok((tcp_socket, addr)) = listener.accept() => {
                let (mut tcp_read, tcp_write) = tcp_socket.into_split();

                info!("Server received new client connection {:?}", &addr);

                let (client_id, mut client_msg);

                // Wait for chat name msg
                let mut name_buf: Vec<u8> = vec![0; 64];
                let mut name: String;

                if let Ok(n) = tcp_read.read(&mut name_buf).await {
                    name_buf.truncate(n);
                    name = String::from_utf8(name_buf.clone()).unwrap();
                    client_id = counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client

                    info!("client_id is {}", client_id);
                } else {
                    error!("Unable to retrieve chat user name");
                    return Ok(());
                }

                // Acquire write lock guard, insert new chat name, using new name if there was a collision
                {
                    let mut lg = chat_names1.write().await;
                    if !lg.insert(name.clone()) { // if collision happened, grab modified handle name
                        name = lg.last_collision().unwrap(); // retrieve updated name, has to be not None

                        // send msg back acknowledging user joined with updated chat name
                        let join_msg = USER_JOINED_ACK.replace("{}", &name);
                        client_msg = MsgType::JoinedAck(client_id, join_msg.into_bytes());

                        // notify others of new join
                        task_tx1.send_timeout(client_msg, Duration::from_millis(75)).await
                            .expect("Unable to tx");
                    }
                }

                // Store client data into clients registry
                {
                    let mut sc = clients.lock().await;
                    sc.entry(client_id).or_insert((addr.clone(), name.clone(), tcp_write));
                }

                let join_msg = USER_JOINED.replace("{}", &name);
                client_msg = MsgType::Joined(client_id, join_msg.into_bytes());

                // notify others of new join
                task_tx1.send_timeout(client_msg, Duration::from_millis(75)).await
                    .expect("Unable to tx");

                let task_tx_ref = task_tx1.clone();
                let clients_ref = Arc::clone(&clients);
                let chat_names_ref = chat_names1.clone();

                let mut nb = name.as_bytes().to_vec();
                // Generate per client msg prefix, e.g. "name: "
                let mut msg_prefix: Vec<u8> = Vec::with_capacity(nb.len() + 2);
                msg_prefix.append(&mut nb);
                msg_prefix.push(b':');
                msg_prefix.push(b' ');

                let mut fr = FramedRead::new(tcp_read, LinesCodec::new_with_max_length(LINES_MAX_LEN));

                // Spawn tokio task to handle server socket reads
                tokio::spawn(async move {
                    loop {
                        // Read lines from server
                        if let Some(value) = fr.next().await {
                            debug!("server received: {:?}", value);

                            match value {
                                Ok(line) if line == "users" => task_tx_ref.send(MsgType::Users(client_id)).await.expect("Unable to tx"),
                                Ok(line) if line.len() > 0 => {
                                    let mut msg: Vec<u8> = Vec::with_capacity(msg_prefix.len() + line.len() + 1);
                                    let mut p = msg_prefix.clone();
                                    let mut line2 = line.into_bytes();
                                    msg.append(&mut p);
                                    msg.append(&mut line2);
                                    msg.push(b'\n');

                                    debug!("Server received client msg {:?}", &msg);

                                    // delegate the broadcast of msg echoing to another block
                                    task_tx_ref.send(MsgType::Message(client_id, msg)).await
                                        .expect("Unable to tx");
                                },
                                Ok(_) => continue,
                                Err(x) => {
                                    debug!("Server Connection closing error: {:?}", x);
                                    break;
                                },
                            }
                        } else {
                            info!("Client connection has closed");
                            let mut remove_name = String::new();
                            let mut leave_msg = vec![];
                            { // keep lock scope small
                                let mut sc = clients_ref.lock().await;
                                if let Some((addr, name, _w)) = sc.remove(&client_id) {
                                    info!("Remote {:?} has closed connection", &addr);
                                    info!("User {} has left", &name);
                                    leave_msg = USER_LEFT.replace("{}", &name)
                                        .into_bytes();
                                    remove_name = name;
                                }
                            }
                            // keep names consistent to reflect client is gone
                            chat_names_ref.write().await.remove(&remove_name);

                            // broadcast that user has left
                            task_tx_ref.send(MsgType::Exited(leave_msg)).await
                                .expect("Unable to tx");

                            break;
                        }
                    }
                });
            }
            Some(message) = local_rx.recv() => {
                // Read data received from server and broadcast to all clients
                // debug!("Local channel msg received {:?}", &message);

                match message {
                    MsgType::Joined(cid, msg) => {
                        // broadcast join msg to all except for cid, then
                        // trigger 'current users' message as well for cid
                        outgoing.broadcast_except(msg, cid).await;
                        task_tx2.send(MsgType::Users(cid)).await.expect("Unable to tx");
                    },
                    MsgType::JoinedAck(cid, msg) |
                    MsgType::MessageSingle(cid, msg) => {//single send to single client cid
                        outgoing.send(cid, msg).await;
                    },
                    MsgType::Message(cid, msg) => { //broadcast to all except for cid
                        outgoing.broadcast_except(msg, cid).await;
                    },
                    MsgType::Exited(msg) => { //broadcast to all
                        outgoing.broadcast(msg).await;
                    },
                    MsgType::Users(cid) => {
                        let users_msg = chat_names2.read().await.to_list();
                        task_tx2.send(MsgType::MessageSingle(cid, users_msg)).await
                            .expect("Unable to tx");
                    }
                }
            }
        }
    }
}
