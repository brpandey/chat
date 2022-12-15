use tokio::sync::mpsc::{Sender, Receiver};

use tracing::{debug, info};

use crate::server_types::MsgType;
use crate::delivery::Delivery;
use crate::names::NamesShared;

pub struct ChannelReceiver {
    local_rx: Receiver<MsgType>,
    local_tx: Sender<MsgType>,
    outgoing: Delivery,
    names: NamesShared,
}

impl ChannelReceiver {

    pub fn new(local_rx: Receiver<MsgType>, outgoing: Delivery, names: NamesShared, local_tx: Sender<MsgType>) -> Self {
        ChannelReceiver {
            local_rx,
            local_tx,
            outgoing,
            names,
        }
    }

    pub fn spawn(mut r: ChannelReceiver) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            r.handle_receive().await;
        })
    }

    pub async fn handle_receive(&mut self) {
        loop {
            if let Some(message) = self.local_rx.recv().await {
                // Read data received from server and broadcast to all clients
                debug!("Local channel msg received {:?}", &message);

                match message {
                    MsgType::Joined(cid, msg) => {
                        // broadcast join msg to all except for cid, then
                        // trigger 'current users' message as well for cid
                        self.outgoing.broadcast_except(msg, cid).await;
                        self.local_tx.send(MsgType::Users(cid)).await.expect("Unable to tx");
                    },
                    MsgType::JoinedAck(cid, msg) |
                    MsgType::MessageSingle(cid, msg) => {//single send to single client cid
                        self.outgoing.send(cid, msg).await;
                    },
                    MsgType::Message(cid, msg) => { //broadcast to all except for cid
                        self.outgoing.broadcast_except(msg, cid).await;
                    },
                    MsgType::Exited(msg) => { //broadcast to all
                        self.outgoing.broadcast(msg).await;
                    },
                    MsgType::Users(cid) => {
                        let users_msg = self.names.read().await.to_list();
                        self.local_tx.send(MsgType::MessageSingle(cid, users_msg)).await
                            .expect("Unable to tx");
                    }
                }
            } else {
                info!("No more channel senders");
                break;
            }
        }
    }
}
