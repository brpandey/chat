use std::collections::HashMap;
use std::sync::Arc;

use tokio::io;
use tokio::sync::{mpsc, Mutex};

use tracing_subscriber::fmt;
use tracing::Level;

use server::delivery::Delivery;
use server::names::NamesShared;
use server::server_types::{MsgType, Registry};
use server::server_listener::ServerListener;
use server::server_channel::ChannelReceiver;

const SERVER: &str = "127.0.0.1:43210";
const BOUNDED_CHANNEL_SIZE: usize = 64;

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact()
        .with_max_level(Level::DEBUG)
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
    // 2) async listening for new clients, or

    let handle = ChannelReceiver::spawn_receive(local_rx, outgoing, chat_names.clone(), local_tx.clone());
    ServerListener::spawn_accept(SERVER.to_owned(), clients, chat_names, local_tx);

    handle.await.unwrap();

    Ok(())
}
