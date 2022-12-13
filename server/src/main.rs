use std::collections::HashMap;
use std::time::Duration;
use std::sync::Arc;
//use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::select;
use tokio::io::{self, AsyncReadExt};
use tokio::net::TcpListener; //, tcp};
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::{info, error, Level};

use server::server::Registry;
use server::delivery::Delivery;
use server::names::NamesShared;
use server::server::MsgType;
use server::client_reader::ClientReader;

const SERVER: &str = "127.0.0.1:4321";

const COUNTER_SEED: usize = 1;
const BOUNDED_CHANNEL_SIZE: usize = 64;

const USER_JOINED: &str = "User {} joined\n";
const USER_JOINED_ACK: &str = "You have successfully joined as {} \n";

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


                let mut nb = name.as_bytes().to_vec();
                // Generate per client msg prefix, e.g. "name: "
                let mut msg_prefix: Vec<u8> = Vec::with_capacity(nb.len() + 2);
                msg_prefix.append(&mut nb);
                msg_prefix.push(b':');
                msg_prefix.push(b' ');

                let reader = ClientReader::new(client_id, tcp_read, task_tx1.clone(),
                                                   Arc::clone(&clients), chat_names1.clone(), msg_prefix);

                ClientReader::spawn(reader);
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
