//! Abstraction to instantiate peer A or B client
use tokio::io;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::builder::PeerClientBuilder as Builder;
use crate::peer_client::PeerClient;
use crate::event_bus::EventBus;
use crate::input_shared::InputShared;
use crate::types::PeerMsg;

use tracing::{debug};

#[derive(Debug)]
pub struct PeerA(pub String, pub String, pub String); // server, client_name, peer_name

#[derive(Debug)]
pub struct PeerB(pub Receiver<PeerMsg>, pub Sender<PeerMsg>, pub String); // client_rx, server_tx, name

#[derive(Debug)]
pub enum Peer {
    PA(PeerA),
    PB(PeerB),
}

impl Peer {
    // Note we consume self here
    pub async fn spawn_ready(self, io_shared: InputShared, eb: EventBus) -> () {
        match self {
            Peer::PA(p) => PeerA::spawn_ready(p.0, p.1, p.2, io_shared, eb).await,
            Peer::PB(p) => PeerB::spawn_ready(p.0, p.1, p.2, io_shared, eb).await,
        }
    }
}

impl PeerA {
    pub async fn spawn_ready(server: String, name: String, peer_name: String,
                             io_shared: InputShared, eb: EventBus) -> () {
        if let Ok(mut client) = PeerA::build(server, name, peer_name, &io_shared, &eb).await {
            // peer A initiates hello since it initiated the session!
            client.send_hello().await;
            client.run(io_shared, eb).await;
        }

        ()
    }

    // client is peer type A which initiates a reaquest to an already running peer B
    // client type A is not connected to the peer B server other than through tcp
    pub async fn build(server: String, name: String, peer_name: String, io_shared: &InputShared, eb: &EventBus)
                       -> io::Result<PeerClient> {

        let client = Builder::new(name, Some(peer_name))
            .connect(&server).await?
            .channels()
            .io_register(io_shared, eb).await
            .build();

        debug!("New peer A, name: {}, peer_name: {}, io_id: {}, successful tcp connect to peer server {:?}",
              &client.name, &client.peer_name.as_ref().unwrap(), client.io_id, &server);

        Ok(client)
    }
}

impl PeerB {
    pub async fn spawn_ready(client_rx: Receiver<PeerMsg>, server_tx: Sender<PeerMsg>, name: String,
                             io_shared: InputShared, eb: EventBus) -> () {
        if let Ok(mut client) = PeerB::build(client_rx, server_tx, name, &io_shared, &eb).await {
            client.run(io_shared, eb).await;
        }

        ()
    }

    // client type B is the interative part of the peer type B server on the same node
    // client type B is connected to the peer type B through channels
    pub async fn build(client_rx: Receiver<PeerMsg>, server_tx: Sender<PeerMsg>,
                         name: String, io_shared: &InputShared, eb: &EventBus)
                         -> io::Result<PeerClient> {

        let client = Builder::new(name, None)
            .connect_local(client_rx, server_tx)
            .channels()
            .io_register(io_shared, eb).await
            .build();

        debug!("New peer B, name: {} io_id: {}, set up local channel to local peer server",
              &client.name, client.io_id);

        Ok(client)
    }
}
