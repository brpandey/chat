use std::collections::HashMap;
use std::sync::Arc;

use tokio::io;
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::Level;

use server::delivery::Delivery;
use server::names::NamesShared;
use server::server_types::{MsgType, Registry};
use server::server_listener::Listener;
use server::server_channel::ChannelReceiver;

const SERVER: &str = "127.0.0.1:4321";
const BOUNDED_CHANNEL_SIZE: usize = 64;

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact()
        .with_max_level(Level::INFO)
        .init();

    // Setup unique registry map and chat names,
    // These are shareable across tasks which may be passed amongst threads
    let clients: Registry = Arc::new(Mutex::new(HashMap::new()));
    let outgoing = Delivery::new(&clients);
    let chat_names: NamesShared = NamesShared::new();

    // Setup server local msg passing channel
    let (local_tx, local_rx) = mpsc::channel::<MsgType>(BOUNDED_CHANNEL_SIZE);

    // Loop to handle
    // 1) async channel read new data
    let receiver = ChannelReceiver::new(local_rx, outgoing, chat_names.clone(), local_tx.clone());
    let handle = ChannelReceiver::spawn(receiver);

    // Loop to handle
    // 2) async listening for new clients, or
    let listener = Listener::new(SERVER, clients, chat_names, local_tx).await;
    Listener::spawn_accept(listener);

    handle.await.unwrap();

    Ok(())
}
