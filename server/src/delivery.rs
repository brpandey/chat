use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::server_types::Registry;

const ALL_CLIENTS: usize = 0;

// handles msg delivery back to client
pub struct Delivery {
    registry: Registry,
}

impl Delivery {
    pub fn new(clients: &Registry) -> Self {
        Delivery {
            registry: Arc::clone(clients),
        }
    }

    pub async fn send(&mut self, cid: usize, msg: Vec<u8>) {
        let mut r = self.registry.lock().await;

        if let Some((_addr, _name, ws)) = r.get_mut(&cid) {
            ws.write_all(&msg).await.expect("unable to write");
        }
    }

    pub async fn broadcast(&mut self, msg: Vec<u8>) {
        self.broadcast_except(msg, ALL_CLIENTS).await;
    }

    pub async fn broadcast_except(&mut self, msg: Vec<u8>, except: usize) {
        // broadcast to client tcp_write sockets stored behind a Mutex
        {
            let mut r = self.registry.lock().await;

            for (k, (_addr, _name, ws)) in r.iter_mut() {
                debug!("About to write to server tcp_socket, msg {:?}", std::str::from_utf8(&msg).unwrap());

                if except == *k { continue } // skip the send to except client id

                let err = format!("Unable to write to tcp socket {:?}", k); // debug
                ws.write_all(&msg).await.expect(&err);
            }
        }
    }
}
