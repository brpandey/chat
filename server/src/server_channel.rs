use tokio::sync::mpsc::{Sender, Receiver};
use tracing::{debug, info};
use protocol::Response;

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
        let mut response: Response;

        loop {
            if let Some(message) = self.local_rx.recv().await {
                // Read data received from server and broadcast to all clients
                debug!("Local channel msg received {:?}", &message);

                match message {
                    MsgType::Joined(cid, msg) => {
                        // broadcast join msg to all except for cid, then
                        // trigger 'current users' message as well for cid
                        response = Response::Notification(msg);
                        self.outgoing.broadcast_except(cid, response).await;
                        self.local_tx.send(MsgType::Users(cid)).await.expect("Unable to tx");
                    },
                    MsgType::JoinedAck(cid, msg) => {
                        response = Response::Notification(msg);
                        self.outgoing.send(cid, response).await;
                    },
                    MsgType::MessageSingle(id, msg) => {//single send to single client cid
                        response = Response::UserMessage{id, msg};
                        self.outgoing.send(id, response).await;
                    },
                    MsgType::Message(id, msg) => { //broadcast to all except for cid
                        response = Response::UserMessage{id, msg};
                        self.outgoing.broadcast_except(id, response).await;
                    },
                    MsgType::Exited(msg) => { //broadcast to all
                        response = Response::Notification(msg);
                        self.outgoing.broadcast(response).await;
                    },
                    MsgType::Users(cid) => {
                        let users_msg = self.names.read().await.to_list();
                        self.local_tx.send(MsgType::MessageSingle(cid, users_msg)).await
                            .expect("Unable to tx");
                    },
                    MsgType::ForkPeerAck(cid, id, name, addr) => {
                        response = Response::ForkPeerAckB{id, name, addr};
                        self.outgoing.send(cid, response).await;
                    },
                    MsgType::PeerUnavailable(cid, name) => {
                        response = Response::PeerUnavailable(name);
                        self.outgoing.send(cid, response).await;
                    }
                }
            } else {
                info!("No more channel senders");
                break;
            }
        }
    }
}
