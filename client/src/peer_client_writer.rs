use tokio::select;
use tokio::net::tcp;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::sync::broadcast::{Receiver as BReceiver};
use tokio_util::codec::{FramedWrite};
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait
use tracing::info;

use protocol::{Ask, ChatCodec};
use crate::types::{PeerMsg};

type FrWrite = FramedWrite<tcp::OwnedWriteHalf, ChatCodec>;

#[derive(Debug)]
pub enum WriteHandle {
    A(FrWrite), // write back using tcp write
    B(Sender<PeerMsg>), // write to local channel to local server
}

#[derive(Debug)]
pub struct PeerWriter {
    write: WriteHandle,
    local_rx: Receiver<Ask>,
    shutdown_rx: BReceiver<u8>,
}

impl PeerWriter {
    pub fn new(write: WriteHandle,
               local_rx: Receiver<Ask>,
               shutdown_rx: BReceiver<u8>)
               -> Self {
        Self {
            write,
            local_rx,
            shutdown_rx,
        }
    }

    pub async fn handle_peer_write(&mut self) {
        loop {
            select! {
                // Read from channel, data received from command line
                Some(msg) = self.local_rx.recv() => {
                    match &mut self.write {
                        WriteHandle::A(ref mut fw) => {
                            info!("handle_peer_write: peer type A {:?}", &msg);
                            fw.send(msg).await.expect("Unable to write to tcp server");
                        },
                        WriteHandle::B(ref server_tx) => {
                            info!("handle_peer_write: peer type B {:?}", &msg);
                            match msg {
                                Ask::Note(m) => {
                                    server_tx.send(PeerMsg::Note(m)).await.expect("Unable to tx");
                                },
                                Ask::Leave(m) => {
                                    server_tx.send(PeerMsg::Leave(m)).await.expect("Unable to tx");
                                },
                                _ => unimplemented!(),
                            }
                        }
                    }
                }
                _ = self.shutdown_rx.recv() => { // exit task if any shutdown received
                    return
                }
            }
        }
    }
}
