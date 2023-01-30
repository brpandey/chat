//! Provide for notification consolidation so client tasks are decoupled from
//! specifics of input msg notification or names data coordination
//! Also allows for client abort handling coordination

use std::sync::Arc;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::broadcast::{self, Sender as BSender, Receiver as BReceiver};
use tokio::sync::mpsc::error::{TrySendError};
use tokio::task;

use crate::input_shared::InputShared;
use crate::types::{InputMsg, EventMsg, InputNotifier};
use crate::peer_set::{PeerSet, PeerNames};

use tracing::error;

const ABORT_ALL: u8 = 1;

pub struct EventBus {
    event_tx: Sender<EventMsg>,
    event_rx: Option<Receiver<EventMsg>>,
    abort_tx: Arc<BSender<u8>>,
    abort_rx: Option<BReceiver<u8>>,
}

impl EventBus {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel::<EventMsg>(64);
        let (abort_tx, abort_rx) = broadcast::channel(1);

        EventBus {
            event_tx,
            event_rx: Some(event_rx),
            abort_tx: Arc::new(abort_tx),
            abort_rx: Some(abort_rx),
        }
    }

    pub fn clone(&self) -> Self {
        EventBus {
            event_tx: self.event_tx.clone(),
            event_rx: None,
            abort_tx: Arc::clone(&self.abort_tx),
            abort_rx: Some(self.abort_tx.subscribe()),
        }
    }


    pub fn notify(&self, msg: EventMsg) -> Result<(), TrySendError<EventMsg>> {
        self.event_tx.try_send(msg)
    }

    /* abort methods */
    pub fn take_abort(&mut self) -> Option<BReceiver<u8>> {
        self.abort_rx.take()
    }

    // Send abort message that clients have access to
    pub fn abort_clients(&self) {
        self.abort_tx.send(ABORT_ALL).expect("Unable to send abort_clients");
    }

    // Handle event msgs that allow the clients (producers) decoupling with the "psuedo"
    // consumer registrations e.g. io_notify, names managment
    pub fn spawn(mut eb: EventBus, io_notify: InputNotifier, io_shared: InputShared,
                 peer_set: PeerSet, names: PeerNames) -> task::JoinHandle<()> {
        let mut event_rx = eb.event_rx.take().unwrap();
        let mut pset = Some(peer_set);

        tokio::spawn(async move {
            loop {
                if let Some(event_msg) = event_rx.recv().await {
                    match event_msg {
                        EventMsg::Spawn(peer) => {
                            if pset.is_some() {
                                pset.as_ref().unwrap().spawn(peer, names.clone(), io_shared.clone(), eb.clone()).await;
                            }
                        }
                        EventMsg::NewSession(id, name) => {
                            if io_notify.try_send(InputMsg::NewSession(id, name)).is_err() {
                                error!("Unable to send input new session msg");
                            }
                        },
                        EventMsg::UpdatedSessionName(id, name) => {
                            if io_notify.try_send(InputMsg::UpdatedSessionName(id, name.clone())).is_err() {
                                error!("Unable to send input update session name msg");
                            } else {
                                names.insert(name).await;
                            }

                        },
                        EventMsg::CloseSession(id, name) => {
                            if io_notify.try_send(InputMsg::CloseSession(id)).is_err() {
                                error!("Unable to send input close session msg");
                            } else {
                                names.remove(&name).await;
                            }
                        },
                        EventMsg::CloseLobby => {
                            pset = None; // disable spawning, drop peer_set reducing arc strong count by 1

                            if io_notify.try_send(InputMsg::CloseLobby).is_err() {
                                error!("Unable to send input close lobby msg");
                            }
                        }
                    }
                }
            }
        })
    }
}
