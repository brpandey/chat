//! Provides an ability to track peer clients that are spawned or are shutdown
//! Prevents duplicate sessions with the same peers
//! Provides for clean shutdown

use std::sync::Arc;
use std::collections::HashSet;

use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::sync::broadcast::{self, Sender as BSender, Receiver as BReceiver};

use crate::types::{PeerMsg, PeerSetError};
use crate::input_shared::InputShared;
use crate::peer_client::{PeerA, PeerB};

type PeerNames = Arc<RwLock<HashSet<String>>>;

use tracing::{debug, /*error*/};

const ABORT_ALL: u8 = 1;

pub struct PeerShared {
    abort_tx: Arc<BSender<u8>>,
    abort_rx: Option<BReceiver<u8>>,
    names: PeerNames, // unique names set of peers connected to
}

impl PeerShared {
    pub fn new() -> Self {
        let names = Arc::new(RwLock::new(HashSet::new())); 
        let (abort_tx, abort_rx) = broadcast::channel(1);

        Self {
            abort_tx: Arc::new(abort_tx),
            abort_rx: Some(abort_rx),
            names,
        }
    }

    pub fn clone(&self) -> Self {
        PeerShared {
            abort_tx: Arc::clone(&self.abort_tx),
            abort_rx: Some(self.abort_tx.subscribe()),
            names: Arc::clone(&self.names),
        }
    }

    /* abort methods */

    pub fn take_abort(&mut self) -> Option<BReceiver<u8>> {
        self.abort_rx.take()
    }

    pub fn abort_all(&self) {
        self.abort_tx.send(ABORT_ALL).expect("Unable to send abort_all");
    }

    /* names methods */

    pub async fn contains(&self, peer_name: &str) -> bool {
        self.names.read().await.contains(peer_name)
    }

    pub async fn insert(&self, peer_name: String) -> bool {
        self.names.write().await.insert(peer_name)
    }

    pub async fn remove(&self, peer_name: &str) -> bool {
        self.names.write().await.remove(peer_name)
    }
}

pub struct PeerSet {
    set: Option<Arc<Mutex<JoinSet<()>>>>,
    shared: Option<PeerShared>
}

impl PeerSet {
    pub fn new() -> Self {
        Self {
            set: Some(Arc::new(Mutex::new(JoinSet::new()))),
            shared: Some(PeerShared::new())
        }
    }

    pub async fn is_empty(&self) -> bool {
        if self.set.is_some() {
            self.set.as_ref().unwrap().lock().await.is_empty()
        } else { // if set has already been taken and is none
            true
        }
    }

    pub fn clone(&self) -> Self {
        PeerSet {
            set: Some(Arc::clone(&self.set.as_ref().unwrap())),
            shared: Some(self.shared.as_ref().unwrap().clone()),
        }
    }

    pub fn get_shared(&mut self) -> PeerShared {
        self.shared.as_mut().unwrap().clone()
    }

    pub async fn join_all(&mut self) -> Result<Option<()>, PeerSetError> {
        // consume arc and lock
        let set = self.set.take().unwrap();
        let lock = Arc::try_unwrap(set).map_err(|_| PeerSetError::ArcUnwrapError)?; // remove Arc layer
        let mut peer_clients = lock.into_inner();

        debug!("clients are {:?}", peer_clients);

        while let Some(res) = peer_clients.join_next().await {
            debug!("peer client completed {:?}", res);
        }

        Ok(None) // no more peer clients left and arc has been consumed
    }

    pub async fn spawn_peer_a(&mut self, server: String, client_name: String, peer_name: String, io_shared: InputShared) {
        // only spawn if peer client name is new (only for type a)
        // avoid case where a node has more than 1 initiated session to the same peer name
        // e.g. avoids case where peer a forks two sessions to the same peer b
        let peer_shared = self.get_shared();
        if peer_shared.insert(peer_name.clone()).await {
            self.set.as_mut().unwrap().lock().await
                .spawn(PeerA::spawn_ready(server, client_name, peer_name, io_shared, peer_shared));
        } else {
            debug!("Unable to spawn as peer client {:?} has already been spawned", &client_name);
        }
    }

    pub async fn spawn_peer_b(&mut self, client_rx: Receiver<PeerMsg>, server_tx: Sender<PeerMsg>,
                              name: String, io_shared: InputShared) {
        // since no peer name provided, unable to store but updated later
        let peer_shared = self.get_shared();
        self.set.as_mut().unwrap().lock().await
            .spawn(PeerB::spawn_ready(client_rx, server_tx, name, io_shared, peer_shared));
    }
}
