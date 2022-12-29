use tokio::net::TcpListener;
use tokio::sync::mpsc;

use std::io::ErrorKind;

use crate::peer_client::PeerClient;
use crate::peer_server_handler::PeerServerHandler;

use crate::peer_types::PeerMsgType;

use tracing::info;

pub struct PeerServerListener;

impl PeerServerListener {
    pub fn spawn_accept(addr: String) {
        let _h = tokio::spawn(async move {

            let result = TcpListener::bind(addr).await;

            if result.is_err() && result.as_ref().unwrap_err().kind() == ErrorKind::AddrInUse {
                info!("Peer server address already in use, can not have more than 1 peer client on the same machine");
                return;
            }

            let listen = result.expect("Unable to bind to server address");

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
                    handler.spawn().await;
                } else {
                    break;
                }
            }
        });
    }
}
