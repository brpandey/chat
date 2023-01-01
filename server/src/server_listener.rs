use std::sync::Arc;
use std::sync::atomic::AtomicU16;

use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;

use crate::server_types::{Registry, MsgType};
use crate::names::NamesShared;
use crate::request_handler::RequestHandler;

use tracing::info;

const COUNTER_SEED: u16 = 1;

pub struct ServerListener;

impl ServerListener {

    pub fn spawn_accept(addr: String, clients: Registry, names: NamesShared, local_tx: Sender<MsgType>) {
        info!("Server starting.. {:?}", &addr);

        // Set up unique counter
        let counter = Arc::new(AtomicU16::new(COUNTER_SEED));

        let _h = tokio::spawn(async move {
            let listener = TcpListener::bind(addr).await.expect("Unable to bind to server address");

            loop {
                if let Ok((tcp_socket, addr)) = listener.accept().await {
                    let (tcp_read, tcp_write) = tcp_socket.into_split();

                    info!("Server received new client connection {:?}", &addr);

                    let reader = RequestHandler::new(tcp_read, local_tx.clone(),
                                                    clients.clone(), names.clone());

                    RequestHandler::spawn(reader, addr, tcp_write, counter.clone());
                } else {
                    info!("Server abnormally exiting .. ");
                    break;
                }
            }
        });
    }
}
