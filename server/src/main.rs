use std::collections::HashMap;
use std::sync::Arc;

use tokio::select;
use tokio::io;
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::{Level};

use server::delivery::Delivery;
use server::names::NamesShared;
use server::server::{MsgType, Registry};
use server::server_listener::ServerTcpListener;

const SERVER: &str = "127.0.0.1:4321";
const BOUNDED_CHANNEL_SIZE: usize = 64;

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact()
        .with_max_level(Level::INFO)
        .init();

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

    let local_tx2 = local_tx.clone(); // clone before spawn

    let listener = ServerTcpListener::new(SERVER, clients, chat_names1, local_tx).await;
    ServerTcpListener::spawn_accept(listener);

    /*
    let receiver = ServerLocalReceiver::new(local_rx, outgoing, chat_names2, local_tx2);
    ServerLocalReceiver::spawn_receive_loop(receiver);
     */

    // Loop to handle either
    // 1) async listening for new clients, or
    // 2) async channel read new data
    loop {
        select! {
            Some(message) = local_rx.recv() => {
                // Read data received from server and broadcast to all clients
                // debug!("Local channel msg received {:?}", &message);

                match message {
                    MsgType::Joined(cid, msg) => {
                        // broadcast join msg to all except for cid, then
                        // trigger 'current users' message as well for cid
                        outgoing.broadcast_except(msg, cid).await;
                        local_tx2.send(MsgType::Users(cid)).await.expect("Unable to tx");
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
                        local_tx2.send(MsgType::MessageSingle(cid, users_msg)).await
                            .expect("Unable to tx");
                    }
                }
            }
        }
    }
}
