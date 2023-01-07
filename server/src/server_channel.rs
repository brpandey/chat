use tokio::sync::mpsc::{Sender, Receiver};
use tracing::{debug, info};
use protocol::Response;

use crate::server_types::MsgType;
use crate::delivery::Delivery;
use crate::names::NamesShared;

pub struct ChannelReceiver;

impl ChannelReceiver {

    pub fn spawn_receive(mut local_rx: Receiver<MsgType>, mut outgoing: Delivery,
                 names: NamesShared, local_tx: Sender<MsgType>) -> tokio::task::JoinHandle<()> {

        tokio::spawn(async move {
            let mut response: Response;

            loop {
                if let Some(message) = local_rx.recv().await {
                    // Read data received from server and broadcast to all clients
                    debug!("Local channel msg received {:?}", &message);

                    match message {
                        MsgType::JoinedOthers(cid, msg) => {
                            // broadcast join msg to all except for cid, then
                            // trigger 'current users' message as well for cid
                            response = Response::Notification(msg);
                            outgoing.broadcast_except(cid, response).await;
                            local_tx.send(MsgType::Users(cid)).await.expect("Unable to tx");
                        },
                        MsgType::JoinedAck(id, name) => {
                            response = Response::JoinNameAck{id, name};
                            outgoing.send(id, response).await;
                        },
                        MsgType::JoinedAckUpdated(cid, msg) => {
                            response = Response::Notification(msg);
                            outgoing.send(cid, response).await;
                        },
                        MsgType::MessageSingle(id, msg) => {//single send to single client cid
                            response = Response::UserMessage{id, msg};
                            outgoing.send(id, response).await;
                        },
                        MsgType::Message(id, msg) => { //broadcast to all except for cid
                            response = Response::UserMessage{id, msg};
                            outgoing.broadcast_except(id, response).await;
                        },
                        MsgType::Exited(msg) => { //broadcast to all
                            response = Response::Notification(msg);
                            outgoing.broadcast(response).await;
                        },
                        MsgType::Users(cid) => {
                            let users_msg = names.read().await.to_list();
                            local_tx.send(MsgType::MessageSingle(cid, users_msg)).await
                                .expect("Unable to tx");
                        },
                        MsgType::ForkPeerAck(cid, pid, name, addr) => {
                            response = Response::ForkPeerAckB{pid, name, addr};
                            outgoing.send(cid, response).await;
                        },
                        MsgType::PeerUnavailable(cid, name) => {
                            response = Response::PeerUnavailable(name);
                            outgoing.send(cid, response).await;
                        }
                    }
                } else {
                    info!("No more channel senders");
                    break;
                }
            }
        })
    }
}
