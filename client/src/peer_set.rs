use std::sync::Arc;
use tokio::io::{self, Error, ErrorKind};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::sync::broadcast::{self, Sender as BSender, Receiver as BReceiver};

use crate::types::PeerMsg;
use crate::input_shared::InputShared;
use crate::peer_client::{PeerA, PeerB};

use tracing::{info, /*error*/};

const ABORT_ALL: u8 = 1;

pub struct PeerSet {
    set: Option<Arc<Mutex<JoinSet<()>>>>,
    abort_tx: Arc<BSender<u8>>,
    #[allow(dead_code)]
    abort_rx: Option<BReceiver<u8>>
}


impl PeerSet {
    pub fn new() -> Self {
        let (abort_tx, abort_rx) = broadcast::channel(1);

        Self {
            set: Some(Arc::new(Mutex::new(JoinSet::new()))),
            abort_tx: Arc::new(abort_tx),
            abort_rx: Some(abort_rx),
        }
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
            set: Some(Arc::clone(&self.set.as_mut().unwrap())),
            abort_tx: Arc::clone(&self.abort_tx),
            abort_rx: None,
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

    pub fn abort_all(&mut self) {
        self.abort_tx.send(ABORT_ALL).expect("Unable to send abort_all");
        info!("sent abort all msg");
    }

    pub async fn spawn_peer_a(&mut self, server: String, client_name: String, peer_name: String, io_shared: InputShared) {
        let abort_rx = self.abort_tx.subscribe();
        self.set.as_mut().unwrap().lock().await
            .spawn(PeerA::spawn_ready(server, client_name, peer_name, io_shared, abort_rx));
    }

    pub async fn spawn_peer_b(&mut self, client_rx: Receiver<PeerMsg>, server_tx: Sender<PeerMsg>,
                              name: String, io_shared: InputShared) {
        let abort_rx = self.abort_tx.subscribe();
        self.set.as_mut().unwrap().lock().await
            .spawn(PeerB::spawn_ready(client_rx, server_tx, name, io_shared, abort_rx));
    }
}
