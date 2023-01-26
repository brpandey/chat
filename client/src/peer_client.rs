//! Main client abstraction to handle running peer client, functionality for reading and writing is offloaded
//! to other modules but are spawned within peer client run umbrella task
//! Distinguishes between peer clients that initiates peer sessions A
//! And receive peer client requests given to peer servers

//! Peer type A's communicate via tcp, whereas peer type B's use a local message channel back to peer server

use tokio::io;
use tokio::select;
use tokio::sync::mpsc::{Sender, Receiver};

use protocol::Ask;
use crate::builder::PeerClientBuilder as Builder;
use crate::types::{PeerMsg, EventMsg, ReaderError, Peer, PeerA, PeerB};
use crate::input_reader::InputReader;
use crate::input_shared::InputShared;
use crate::event_bus::EventBus;

use tracing::{debug, error};

const SHUTDOWN: u8 = 1;
const SHUTDOWN_ABORT: u8 = 2;

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


//#[derive(Debug)]
pub struct PeerClient {
    name: String,
    peer_name: Option<String>,
    io_id: u16,
    local_tx: Option<Sender<Ask>>,
    builder: Builder,
}

impl PeerClient {
    pub fn new(name: String, peer_name: Option<String>, io_id: u16, local_tx: Option<Sender<Ask>>, builder: Builder) -> Self {
        Self {
            name,
            peer_name,
            io_id,
            local_tx,
            builder,
        }
    }

    async fn send_hello(&mut self) {
        let msg = Ask::Hello(self.name.clone().into_bytes());
        self.local_tx.as_mut().unwrap().send(msg).await.expect("Unable to tx");
    }

    async fn run(&mut self, io_shared: InputShared, mut eb: EventBus) {
        let mut input_rx = io_shared.get_receiver();

        // grab builder fields
        let (mut reader, mut writer, shutdown_tx, mut shutdown_rx) = self.builder.take_fields();
        let shutdown_tx2 = shutdown_tx.clone();
        let mut shutdown_rx2 = shutdown_tx2.subscribe();

        let eb2 = eb.clone();

        // grab self field data
        let name = self.name.clone();
        let io_id = self.io_id;
        let local_tx = self.local_tx.clone().unwrap();

        let peer_server_read_handle = tokio::spawn(async move {
            reader.handle_peer_read(io_id, eb2).await
        });

        let _peer_write_handle = tokio::spawn(async move {
            writer.handle_peer_write().await;
        });

        // Use current thread to loop and grab data from command line
        let cmd_line_handle = tokio::spawn(async move {
            loop {
                select! {
                    input = async {
                        let req = InputReader::read(io_id, &mut input_rx, &io_shared).await?
                            .and_then(|m| PeerClient::parse_input(&name, m));

                        if req.is_some() {
                            local_tx.send(req.unwrap()).await.expect("Unable to tx");
                        }

                        Ok::<_, ReaderError>(())
                    } => {
                        if input.is_err() { // if input handler has received a terminate
                            shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                            return
                        }
                    }
                    _ = shutdown_rx.recv() => { // exit task if shutdown received
                        return
                    }
                }
            }
        });

        let mut abort_rx = eb.take_abort().unwrap();
        let peer_name: Option<String>;

        loop {
            select! {
                Ok(_) = abort_rx.recv() => {
                    shutdown_tx2.send(SHUTDOWN_ABORT).expect("Unable to send shutdown");
                }
                _ = shutdown_rx2.recv() => {
                    peer_name = peer_server_read_handle.await.unwrap();
                    cmd_line_handle.await.unwrap();
                    break;
                }
            }
        }

        // remove peer name from peer_set using a valid peer_name
        let pname = self.peer_name.take().or(peer_name).unwrap(); // {
//            peer_shared.remove(&pn).await;
//        }

        // given read task is finished (e.g. through \leave or disconnect) switch back to lobby session
        if eb.notify(EventMsg::CloseSession(io_id, pname)).is_err() {
            error!("Unable to send close sesion msg");
        }


        debug!("Peer client Exiting!");
    }

    // Handles peer client commands and peer text input
    pub fn parse_input(name: &str, line: String) -> Option<Ask> {
        match line.as_str() {
            "\\leave" | "\\quit" => {
                println!("Private session ended by {}", name);
                return Some(Ask::Leave(name.as_bytes().to_vec()));
            },
            l => {
                let header = format!("<{}> ", name).into_bytes();
                let msg = InputReader::interleave_newlines(l, Some(header));
                return Some(Ask::Note(msg))
            },
        }
    }
}
