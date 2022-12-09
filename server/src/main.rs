use std::collections::{HashMap, HashSet};
use std::sync::Arc;
//use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::select;
use tokio::io::{self, AsyncReadExt};
use tokio::net::{TcpListener}; //, tcp};
use tokio::sync::{mpsc, Mutex, RwLock};

use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_stream::StreamExt;

use tracing_subscriber::fmt;
use tracing::{info, debug, error, Level};

use server::server::Registry;
use server::delivery::Delivery;

const SERVER: &str = "127.0.0.1:4321";

const COUNTER_SEED: usize = 1;
const LINES_MAX_LEN: usize = 256;
const BOUNDED_CHANNEL_SIZE: usize = 64;

const CURRENT_USERS_MSG: &str = "Current users online are: ";
const USER_JOINED: &str = "User {} joined\n";
const USER_LEFT: &str = "User {} has left\n";

#[derive(Debug)]
pub enum MsgType {
    Joined(usize, Vec<u8>),
    Message(usize, Vec<u8>),
    MessageSingle(usize, Vec<u8>),
    Exited(Vec<u8>),
    Users(usize),
}

//current chat names
type Names = Arc<RwLock<HashSet<Vec<u8>>>>;

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact()
        .with_max_level(Level::INFO)
        .init();

    info!("Server starting.. {:?}", &SERVER);

    let listener = TcpListener::bind(SERVER).await.expect("Unable to bind to server address");

    // Setup registry map, allow it to be shared across tasks, which may be passed along threads
    // Setup names hashset, allow it to be shared across tasks...

    // Setup local msg passing channel
    // Set up unique counter
    // clone tx end of local channel

    let clients: Registry = Arc::new(Mutex::new(HashMap::new()));
    let mut outgoing = Delivery::new(&clients);

    let chat_names1 = Arc::new(RwLock::new(HashSet::new()));
    let chat_names2 = Arc::clone(&chat_names1);

    let (local_tx, mut local_rx) = mpsc::channel::<MsgType>(BOUNDED_CHANNEL_SIZE);
    let task_tx1 = local_tx.clone(); // clone before spawn
    let task_tx2 = local_tx.clone(); // clone before spawn

    let counter = Arc::new(AtomicUsize::new(COUNTER_SEED));

    // Loop to handle either
    // 1) async listening for new clients, or
    // 2) async channel read new data
    loop {
        select! {
            Ok((tcp_socket, addr)) = listener.accept() => {
                let (mut tcp_read, tcp_write) = tcp_socket.into_split();

                info!("Server received new client connection {:?}", &addr);

                let (client_id, client_msg);

                // Wait for chat name msg
                let mut name_buf: Vec<u8> = vec![0; 64];

                if let Ok(n) = tcp_read.read(&mut name_buf).await {
                    name_buf.truncate(n);
                    client_id = counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client

                    let join_msg = USER_JOINED.replace("{}", &std::str::from_utf8(&name_buf).unwrap());
                    client_msg = MsgType::Joined(client_id, join_msg.into_bytes());
                    info!("client_id is {}", client_id);
                } else {
                    error!("Unable to retrieve chat user name");
                    return Ok(());
                }

                // Store (k,v) or (client specific addr, chat name, write tcp socket) in map
                // Need to wrap in Arc/Mutex since it is shared out to 
                // both tokio tasks
                {
                    let mut sc = clients.lock().await;
                    sc.entry(client_id).or_insert((addr.clone(), name_buf.clone(), tcp_write));
                }

                // insert new chat name
                chat_names1.write().await.insert(name_buf.clone());

                // notify others of new join
                task_tx1.send(client_msg).await.expect("Unable to tx");

                let task_tx_ref = task_tx1.clone();
                let clients_ref = Arc::clone(&clients);
                let chat_names_ref = Arc::clone(&chat_names1);

                // Generate per client msg prefix, e.g. "name: "
                let mut msg_prefix: Vec<u8> = Vec::with_capacity(name_buf.len() + 2);
                msg_prefix.append(&mut name_buf);
                msg_prefix.push(b':');
                msg_prefix.push(b' ');

                let mut fr = FramedRead::new(tcp_read, LinesCodec::new_with_max_length(LINES_MAX_LEN));

                // Spawn tokio task to handle server socket reads
                tokio::spawn(async move {
                    loop {
                        // Read lines from server
                        if let Some(value) = fr.next().await {
                            debug!("serve value is : {:?}", value);

                            match value {
                                Ok(line) if line == "users" => task_tx_ref.send(MsgType::Users(client_id)).await.expect("Unable to tx"),
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
                            let mut remove_name = vec![];
                            let mut leave_msg = vec![];
                            { // keep lock scope small
                                let mut sc = clients_ref.lock().await;
                                if let Some((addr, name, _w)) = sc.remove(&client_id) {
                                    info!("Remote {:?} has closed connection", &addr);
                                    info!("User {} has left", std::str::from_utf8(&name).unwrap());
                                    leave_msg = USER_LEFT.replace("{}", &std::str::from_utf8(&name).unwrap())
                                        .into_bytes();
                                    remove_name = name;
                                }
                            }
                            // keep names consistent now that client is gone
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
                debug!("Local channel msg received {:?}", &message);

                let chat_names_ref2 = Arc::clone(&chat_names2);

                match message {
                    MsgType::Joined(cid, msg) => {
                        //broadcast join msg to all except for cid, then
                        // trigger 'current users' message as well for cid
                        outgoing.broadcast_except(msg, cid).await;
                        task_tx2.send(MsgType::Users(cid)).await.expect("Unable to tx");
                    },
                    MsgType::MessageSingle(cid, msg) => {//single send to single client
                        outgoing.send(cid, msg).await;
                    },
                    MsgType::Message(cid, msg) => { //broadcast to all except for cid
                        outgoing.broadcast_except(msg, cid).await;
                    },
                    MsgType::Exited(msg) => { //broadcast to all
                        outgoing.broadcast(msg).await;
                    },
                    MsgType::Users(cid) => {
                        let users_msg = get_names(chat_names_ref2).await;
                        task_tx2.send(MsgType::MessageSingle(cid, users_msg)).await
                            .expect("Unable to tx");
                    }
                }
            }
        }
    }
}


// todo: function should go into an impl block for a type e.g. Names
async fn get_names(chat_names: Names) -> Vec<u8> {
    // construct list of users currently online by
    // grabbing names from chat names which is behind a RWLock
    let mut users_online: Vec<u8>;
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
    users_online
}
