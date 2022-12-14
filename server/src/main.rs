use std::collections::HashMap;
use std::sync::Arc;

use tokio::io;
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::Level;

use server::delivery::Delivery;
use server::names::NamesShared;
use server::server::{MsgType, Registry};
use server::server_listener::ServerTcpListener;
use server::server_channel::ServerLocalReceiver;

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
    let outgoing = Delivery::new(&clients);

    // Setup two shared copies of a set of unique chat names
    // which are shared across tokio tasks
    let chat_names1: NamesShared = NamesShared::new();
    let chat_names2: NamesShared = chat_names1.clone();

    // Setup local msg passing channel
    // clone tx end of local channel

    let (local_tx, local_rx) = mpsc::channel::<MsgType>(BOUNDED_CHANNEL_SIZE);

    let local_tx2 = local_tx.clone(); // clone before spawn

    // Loop to handle
    // 1) async listening for new clients, or
    let listener = ServerTcpListener::new(SERVER, clients, chat_names1, local_tx).await;
    ServerTcpListener::spawn_accept(listener);

    // Loop to handle
    // 2) async channel read new data
    let receiver = ServerLocalReceiver::new(local_rx, outgoing, chat_names2, local_tx2);
    let handle = ServerLocalReceiver::spawn(receiver);

    handle.await.unwrap();

    Ok(())
}
