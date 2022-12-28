use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::peer_client::PeerClient;
use crate::peer_server_handler::PeerServerHandler;

use crate::peer_types::PeerMsgType;

use tracing::info;

pub struct PeerServerListener;

impl PeerServerListener {
    pub fn spawn_accept(addr: String) {
        let _h = tokio::spawn(async move {

            let listen = TcpListener::bind(addr).await.expect("Unable to bind to server address");

            loop {
                if let Ok((tcp_socket, addr)) = listen.accept().await {
                    let (tcp_read, tcp_write) = tcp_socket.into_split();

                    info!("Server received new client connection {:?}", &addr);

                    // create msg queues between peer client and peer server handler
                    // so they can easily communicate with each other
                    let (client_tx, client_rx) = mpsc::channel::<PeerMsgType>(64);
                    let (server_tx, server_rx) = mpsc::channel::<PeerMsgType>(64);

                    PeerClient::spawn_b(client_rx, server_tx);

                    let mut handler = PeerServerHandler::new(tcp_read, tcp_write, client_tx, server_rx);
                    handler.spawn();
                } else {
                    break;
                }
            }
        });
    }
}
