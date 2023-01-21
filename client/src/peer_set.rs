use std::sync::Arc;
use std::collections::HashSet;

use tokio::io::{self, Error, ErrorKind};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::sync::broadcast::{self, Sender as BSender, Receiver as BReceiver};

use crate::types::PeerMsg;
use crate::input_shared::InputShared;
use crate::peer_client::{PeerA, PeerB};

use tracing::{info, /*error*/};

const ABORT_ALL: u8 = 1;

pub struct PeerSetShared {
    abort_rx: Option<BReceiver<u8>>,
    names: Arc<RwLock<HashSet<String>>>,
}

impl PeerSetShared {
    pub fn take_abort(&mut self) -> Option<BReceiver<u8>> {
        self.abort_rx.take()
    }

    pub async fn contains(&self, peer_name: &str) -> bool {
        self.names.read().await.contains(peer_name)
    }

    pub async fn remove(&mut self, peer_name: &str) -> bool {
        info!("name {:?} removed from peer_set names", peer_name);
        self.names.write().await.remove(peer_name)
    }
}

pub struct PeerSet {
    set: Option<Arc<Mutex<JoinSet<()>>>>,
    names: Arc<RwLock<HashSet<String>>>, // unique names set of peers connected to
    abort_tx: Arc<BSender<u8>>,
    #[allow(dead_code)]
    abort_rx: Option<BReceiver<u8>>
}

impl PeerSet {
    pub fn new() -> Self {
        let (abort_tx, abort_rx) = broadcast::channel(1);

        Self {
            set: Some(Arc::new(Mutex::new(JoinSet::new()))),
            names: Arc::new(RwLock::new(HashSet::new())),
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
            names: Arc::clone(&self.names),
            abort_tx: Arc::clone(&self.abort_tx),
            abort_rx: None,
        }
    }

    pub fn get_shared(&mut self) -> PeerSetShared {
        PeerSetShared {
            abort_rx: Some(self.abort_tx.subscribe()),
            names: Arc::clone(&self.names),
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
        // only spawn if peer client name is new (only for type a)
        // avoid case where a node has more than 1 initiated session to the same peer name
        // e.g. avoids case where peer a forks two sessions to the same peer b
        if self.names.write().await.insert(peer_name.clone()) {
            //            let abort_rx = self.abort_tx.subscribe();
            let ps_shared = self.get_shared();
            self.set.as_mut().unwrap().lock().await
                .spawn(PeerA::spawn_ready(server, client_name, peer_name, io_shared, ps_shared));
        } else {
            info!("Unable to spawn as peer client {:?} has already been spawned", &client_name);
        }
    }

    pub async fn spawn_peer_b(&mut self, client_rx: Receiver<PeerMsg>, server_tx: Sender<PeerMsg>,
                              name: String, io_shared: InputShared) {
        // since no peer name provided unable to store such peer name
        // could have a scenario where peer a and peer b have two duplicate sessions
        // e.g. peer a connects to peer b, and peer b connects to peer a -- unlikely but possible
        let ps_shared = self.get_shared();
//        let abort_rx = self.abort_tx.subscribe();
        self.set.as_mut().unwrap().lock().await
            .spawn(PeerB::spawn_ready(client_rx, server_tx, name, io_shared, ps_shared));
    }
}
