use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize};

use tokio::select;
use tokio::io;
use tokio::net::TcpListener; //, tcp};
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::{info, Level};

use server::server::Registry;
use server::delivery::Delivery;
use server::names::NamesShared;
use server::server::MsgType;
use server::client_reader::ClientReader;

const SERVER: &str = "127.0.0.1:4321";

const COUNTER_SEED: usize = 1;
const BOUNDED_CHANNEL_SIZE: usize = 64;


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
                let (tcp_read, tcp_write) = tcp_socket.into_split();

                info!("Server received new client connection {:?}", &addr);

                let reader = ClientReader::new(tcp_read, task_tx1.clone(),
                                               Arc::clone(&clients), chat_names1.clone());

                ClientReader::spawn(reader, addr, tcp_write, Arc::clone(&counter));
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
