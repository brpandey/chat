use tokio::net::TcpListener;
use tokio::sync::mpsc;

use std::io::ErrorKind;

use crate::peer_client::PeerClient;
use crate::peer_server_request_handler::PeerServerRequestHandler;
use crate::peer_types::PeerMsgType;
use crate::input_handler::InputShared;

use tracing::info;

pub const PEER_SERVER: &str = "127.0.0.1";
const PEER_SERVER_PORT0: u16 = 43310;
const PEER_SERVER_PORT1: u16 = 43311;
const PEER_SERVER_PORT2: u16 = 43312;
const PEER_SERVER_PORT3: u16 = 43313;


pub struct PeerServerListener;

impl PeerServerListener {
    pub fn spawn_accept(addr: String, name: String, io_shared: InputShared) {
        let _h = tokio::spawn(async move {

            info!("Client peer server starting {:?}", &addr);

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

                    // For each new peer that wants to connect with this node e.g. N1
                    // spawn a separate peer client of type B that locally communicates with peer server
                    // of type B
                    PeerClient::spawn_b(client_rx, server_tx, name.clone(), io_shared.clone());

                    let mut handler = PeerServerRequestHandler::new(tcp_read, tcp_write, client_tx, server_rx, name.clone());
                    handler.spawn().await;
                } else {
                    break;
                }
            }
        });
    }
}


/* Utility function(s) */

// Stagger port value given num
pub fn peer_port(id: u16) -> u16 {
    match id % 4 {
        0 => PEER_SERVER_PORT0,
        1 => PEER_SERVER_PORT1,
        2 => PEER_SERVER_PORT2,
        3 => PEER_SERVER_PORT3,
        _ => PEER_SERVER_PORT0,
    }
}
