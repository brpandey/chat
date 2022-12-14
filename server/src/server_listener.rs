use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use tokio::io;
use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;

use crate::server::{Registry, MsgType};
use crate::names::NamesShared;
use crate::client_reader::ClientReader;

use tracing::{info};

const COUNTER_SEED: usize = 1;

pub struct ServerTcpListener {
    listener: TcpListener,
    clients: Registry,
    names: NamesShared,
    local_tx: Sender<MsgType>,
    counter: Arc<AtomicUsize>,
}

impl ServerTcpListener {
    pub async fn new(addr: &str, clients: Registry, names: NamesShared, local_tx: Sender<MsgType>) -> Self {
        info!("Server starting.. {:?}", &addr);

        let listener = TcpListener::bind(addr).await.expect("Unable to bind to server address");

        // Set up unique counter
        let counter = Arc::new(AtomicUsize::new(COUNTER_SEED));

        Self {
            listener,
            clients,
            names,
            local_tx,
            counter
        }
    }

    pub fn spawn_accept(mut l: ServerTcpListener) {
        let _h = tokio::spawn(async move {
            l.handle_accept().await.expect("accept terminated");
        });
    }

    pub async fn handle_accept(&mut self) -> io::Result<()> {
        loop {
            let (tcp_socket, addr) = self.listener.accept().await?;
            let (tcp_read, tcp_write) = tcp_socket.into_split();

            info!("Server received new client connection {:?}", &addr);

            let reader = ClientReader::new(tcp_read,
                                           self.local_tx.clone(),
                                           self.clients.clone(),
                                           self.names.clone());

            ClientReader::spawn(reader, addr, tcp_write, self.counter.clone());
        }
    }
}
