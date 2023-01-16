use std::sync::Arc;
use tokio::io::{self, Error, ErrorKind};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::sync::mpsc::{Sender, Receiver};

use crate::peer_types::PeerMsgType;
use crate::input_shared::InputShared;
use crate::peer_client::PeerClient;

use tracing::{info, /*error*/};

pub struct PeerSet {
    set: Option<Arc<Mutex<JoinSet<()>>>>,
}

impl PeerSet {
    pub fn new() -> Self {
        Self { set: Some(Arc::new(Mutex::new(JoinSet::new()))) }
    }

    pub async fn is_empty(&self) -> bool {
        if self.set.is_some() {
            self.set.as_ref().unwrap().lock().await.is_empty()
        } else { // if set has already been taken and is none
            true
        }
    }

    pub fn clone(&mut self) -> Self {
        PeerSet {
            set: Some(Arc::clone(&self.set.as_mut().unwrap()))
        }
    }

    pub async fn join_all(&mut self) -> io::Result<Option<()>> {
        // consume arc and lock
        let set = self.set.take().unwrap();
        let try_arc = Arc::try_unwrap(set); // remove Arc layer

        let mut peer_clients = if try_arc.is_ok() {
            let lock = try_arc.unwrap();
            lock.into_inner() // with Arc layer removed, consume Mutex lock returning inner data
        } else {
            return Err(Error::new(ErrorKind::Other, "arc joinset has other active references thus unable to unwrap"))
        };

        info!("clients are {:?}", peer_clients);

        while let Some(res) = peer_clients.join_next().await {
            info!("peer client completed {:?}", res);
        }

        Ok(None) // no more peer clients left and arc has been consumed
    }

    pub async fn spawn_a(&mut self, server: String, client_name: String, peer_name: String, io_shared: InputShared) {
        self.set.as_mut().unwrap().lock().await
            .spawn(PeerClient::nospawn_a(server, client_name, peer_name, io_shared));
    }

    pub async fn spawn_b(&mut self, client_rx: Receiver<PeerMsgType>, server_tx: Sender<PeerMsgType>,
                         name: String, io_shared: InputShared) {
        self.set.as_mut().unwrap().lock().await
            .spawn(PeerClient::nospawn_b(client_rx, server_tx, name, io_shared));
    }
}
