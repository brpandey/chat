//! Provides an ability to track peer clients that are spawned or are shutdown
//! Prevents duplicate sessions with the same peers
//! Provides for clean shutdown

use std::sync::Arc;
use std::collections::HashSet;

use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinSet;

use crate::types::PeerSetError;
use crate::peer_director::Peer;
use crate::input_shared::InputShared;
use crate::event_bus::EventBus;

type Names = Arc<RwLock<HashSet<String>>>;

use tracing::{debug};

pub struct PeerNames {
    names: Names, // unique names set of peers that client is connected to
}

impl PeerNames {
    pub fn new() -> Self {
        let names = Arc::new(RwLock::new(HashSet::new())); 

        Self {
            names,
        }
    }

    pub fn clone(&self) -> Self {
        PeerNames {
            names: Arc::clone(&self.names),
        }
    }

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
}

impl PeerSet {
    pub fn new() -> Self {
        Self {
            set: Some(Arc::new(Mutex::new(JoinSet::new()))),
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
        }
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

    // Spawn either peer type a or peer type b,
    // checking if peer_name (p.2) has already been seen
    pub async fn spawn(&self, peer: Peer,
                       names: PeerNames,
                       io_shared: InputShared,
                       eb: EventBus) {
        let spawn;

        match peer {
            // only spawn if peer client name is new (only for type a)
            // avoid case where a node has more than 1 initiated session to the same peer name
            // e.g. avoids case where peer a forks two sessions to the same peer b
            Peer::PA(ref p) => {
                spawn = names.insert(p.2.clone()).await;
                if !spawn {
                    debug!("Unable to spawn as peer client {:?} has already been spawned", &p.1);
                }
            },
            // since no peer name provided, unable to store but updated later from peer client
            Peer::PB(_) => spawn = true,
        }

        if spawn {
            self.set.as_ref().unwrap().lock().await.spawn(peer.spawn_ready(io_shared, eb));
        }
    }
}
