use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio_util::codec::Encoder;

use tracing::{debug, error};
use bytes::BytesMut;

//use tokio_util::codec::FramedWrite;
//use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait

use protocol::{ChatCodec, Response};

use crate::server_types::Registry;

const ALL_CLIENTS: u16 = 0;

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

    pub async fn send(&mut self, cid: u16, msg: Response) {
        let mut r = self.registry.lock().await;

        if let Some((_addr, _name, ws)) = r.get_mut(&cid) {
            let mut chat = ChatCodec;
            let mut bm = BytesMut::new();

            if let Err(e) = chat.encode(msg, &mut bm) {
                error!("Unable to encode msg properly {}", e);
                return
            }

            let v = bm.to_vec();
            ws.write_all(&v).await.expect("unable to write");
        }
    }

    pub async fn broadcast(&mut self, msg: Response) {
        self.broadcast_except(ALL_CLIENTS, msg).await;
    }

    pub async fn broadcast_except(&mut self, except_cid: u16, msg: Response) {
        // broadcast to client tcp_write sockets stored behind a Mutex

        let mut chat = ChatCodec;
        let mut bm = BytesMut::new();

        debug!("About to write to server tcp_socket, msg {:?}", &msg);

        if let Err(e) = chat.encode(msg, &mut bm) {
            error!("Unable to encode msg properly {}", e);
            return
        }

        let v = bm.to_vec();

        {
            let mut r = self.registry.lock().await;

            for (k, (_addr, _name, ws)) in r.iter_mut() {
                if except_cid == *k { continue } // skip the send to except client id
                let err = format!("Unable to write to tcp socket {:?}", k); // debug
                ws.write_all(&v).await.expect(&err);
            }
        }
    }

}
