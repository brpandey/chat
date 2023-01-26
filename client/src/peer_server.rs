//! Peer server accepts new tcp requests for new peer sessions from initiating peer clients (A)
//! Peer server is connected with a new peer client type B via message channels upon new request handling
//! Subsequent newly spawned peer clients b allow an exisiting user to dynamically speak to interested
//! peers in a 1-to-1 non-broadcast manner via tcp session

use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::sync::broadcast::Receiver as BReceiver;
use tokio::sync::mpsc;

use crate::types::{PeerMsg, EventMsg, Peer, PeerB};
use crate::peer_server_request_handler::PeerServerRequestHandler;
use crate::event_bus::EventBus;
use tracing::{debug, info};

pub struct PeerServerListener;

impl PeerServerListener {
    pub fn spawn_accept(addr: String, name: String, mut shutdown_rx: BReceiver<u8>, eb: EventBus)
                        -> JoinHandle<()> {

        tokio::spawn(async move {
            debug!("Client peer server starting {:?}", &addr);

            let result = TcpListener::bind(addr).await;

            if result.is_err() && result.as_ref().unwrap_err().kind() ==
                std::io::ErrorKind::AddrInUse {
                info!("Peer server address already in use, can not have more than 1 peer client on the same machine");
                return;
            }
            let listen = result.expect("Unable to bind to server address");

            loop {
                tokio::select! {
                    Ok((tcp_socket, addr)) = listen.accept() => {
                        let (tcp_read, tcp_write) = tcp_socket.into_split();

                        debug!("Server received new client connection {:?}", &addr);

                        // create msg queues between peer client and peer server handler
                        // so they can easily communicate with each other
                        let (client_tx, client_rx) = mpsc::channel::<PeerMsg>(64);
                        let (server_tx, server_rx) = mpsc::channel::<PeerMsg>(64);

                        // For each new peer that wants to connect with this node e.g. N1
                        // spawn a separate peer client of type B that locally communicates with peer server
                        let b = PeerB(client_rx, server_tx, name.clone());
                        eb.notify(EventMsg::Spawn(Peer::PB(b))).expect("Unable to send event msg");

                        let mut handler = PeerServerRequestHandler::new(tcp_read, tcp_write, client_tx, server_rx, name.clone());
                        handler.spawn();
                    }
                    _ = shutdown_rx.recv() => {
                        debug!("Peer server received shutdown, returning!");
                        return;
                    }
                }
            }
        })
    }
}


pub const PEER_SERVER: &str = "127.0.0.1";
const PEER_SERVER_PORT0: u16 = 43310;
const PEER_SERVER_PORT1: u16 = 43311;
const PEER_SERVER_PORT2: u16 = 43312;
const PEER_SERVER_PORT3: u16 = 43313;

pub struct PeerServer;

impl PeerServer {
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

    pub fn stagger_address_port(mut addr: std::net::SocketAddr, id: u16) -> String {
        // drop current port of addr
        // (since this is used already)
        // add a staggered port to addr
        addr.set_port(PeerServer::peer_port(id));
        format!("{}", &addr)
    }
}
