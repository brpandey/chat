//! Builder to simplify client (regular client and peer client) construct
//! Provides routines to grab values for future e.g. shutdown handles

use async_trait::async_trait;

use tokio::io;
use tokio::sync::{broadcast};
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio::sync::broadcast::Receiver as BReceiver;
use tokio::sync::broadcast::Sender as BSender;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio::net::{tcp, TcpStream};

type FrRead = FramedRead<tcp::OwnedReadHalf, ChatCodec>;
type FrWrite = FramedWrite<tcp::OwnedWriteHalf, ChatCodec>;

pub type PeerFields = (PeerReader, PeerWriter, BSender<u8>, BReceiver<u8>);

use protocol::{Request, Ask, ChatCodec};

use crate::client::Client;
use crate::types::{PeerMsg, EventMsg};
use crate::peer_client::PeerClient;
use crate::peer_client_reader::{ReadHandle, PeerReader};
use crate::peer_client_writer::{WriteHandle, PeerWriter};
use crate::input_shared::InputShared;
use crate::event_bus::EventBus;

use tracing::{/*info,*/ debug, error};

#[async_trait]
pub trait Builder {
    type ClientType;

    async fn setup_connection(&mut self, ct: ConnectType) -> io::Result<()>;
    fn setup_channels(&mut self);
    async fn setup_io(&mut self, io_shared: &InputShared, eb: Option<&EventBus>);
    fn build(self) -> Self::ClientType;
}

pub struct PeerClientBuilder {
    name: Option<String>,
    peer_name: Option<String>,
    rh: Option<ReadHandle>,
    wh: Option<WriteHandle>,
    reader: Option<PeerReader>,
    writer: Option<PeerWriter>,
    shutdown_tx: Option<BSender<u8>>,
    shutdown_rx: Option<BReceiver<u8>>,
    local_tx: Option<Sender<Ask>>,
    io_id: u16,
}

pub enum ConnectType {
    ServerName(String),
    Local(Receiver<PeerMsg>, Sender<PeerMsg>)
}

// Builder provides a simpler, reusable and pipelined interface when constructing Peer Client structure
#[async_trait]
impl Builder for PeerClientBuilder {
    type ClientType = PeerClient;

    // Used for PeerClient type A to connect to remote server
    async fn setup_connection(&mut self, ct: ConnectType) -> io::Result<()> {
        match ct {
            ConnectType::ServerName(server_name) => {
                let client = TcpStream::connect(server_name).await
                    .map_err(|e| { error!("Unable to connect to server"); e })?;
                // split tcpstream so we can hand off to r & w tasks
                let (tcp_read, tcp_write) = client.into_split();

                // stash r+w handles
                self.rh = Some(ReadHandle::A(FramedRead::new(tcp_read, ChatCodec)));
                self.wh = Some(WriteHandle::A(FramedWrite::new(tcp_write, ChatCodec)));
            },
            // Used for PeerClient type B to link up with local server
            ConnectType::Local(client_rx, server_tx) => {
                // stash r+w handles
                self.rh = Some(ReadHandle::B(client_rx));
                self.wh = Some(WriteHandle::B(server_tx));
            }
        }

        Ok(())
    }


    // Setup channels support
    fn setup_channels(&mut self) {
        // 1) communication between cmd line read and peer write
        // 2) for all peer client tasks shutdown (e.g. user has disconnected or peer user has disconnected)
        let (local_tx, local_rx) = mpsc::channel::<Ask>(64); // 1
        let (sd_tx, sd_rx) = broadcast::channel(16); // 2
        self.local_tx = Some(local_tx);

        // stash peer reader and writers
        self.reader = Some(PeerReader::new(self.rh.take().unwrap(), sd_tx.clone(), sd_tx.subscribe()));
        self.writer = Some(PeerWriter::new(self.wh.take().unwrap(), local_rx, sd_rx));

        // stash shutdown channel handles
        self.shutdown_rx = Some(sd_tx.subscribe());
        self.shutdown_tx = Some(sd_tx);

    }

    // Register new io id and notify if peer name available
    async fn setup_io(&mut self, io_shared: &InputShared, eb: Option<&EventBus>) {
        self.io_id = io_shared.get_next_id().await;
        if self.peer_name.is_some() && eb.is_some() {
            let bus = eb.unwrap();

            // notify new peer client session (regular client session is default so no notification)
            bus.notify(
                EventMsg::NewSession(self.io_id, self.peer_name.as_ref().unwrap().clone())
            )
            .expect("Unable to send event msg");
        }
    }

    // Build target structure (PeerClient) from builder and moving builder into target
    // NOTE: Compiler allows mut self here but not in trait defn - probably an unenforced compiler error
    fn build(mut self) -> PeerClient {
        PeerClient::new(
            self.name.take().unwrap(),
            self.peer_name.take(),
            self.io_id,
            self.local_tx.take(),
            Some(self.take_fields()),
        )
    }
}

impl PeerClientBuilder {
    pub fn new(name: String, peer_name: Option<String>) -> Self {
        Self {
            name: Some(name),
            peer_name,
            rh: None,
            wh: None,
            reader: None,
            writer: None,
            shutdown_tx: None,
            shutdown_rx: None,
            local_tx: None,
            io_id: 0,
        }
    }

    // Extracts relevant fields into single tuple
    pub fn take_fields(mut self) -> PeerFields {
        let reader = self.reader.take().unwrap();
        let writer = self.writer.take().unwrap();
        let shutdown_tx = self.shutdown_tx.take().unwrap();
        let shutdown_rx = self.shutdown_rx.take().unwrap();

        (reader, writer, shutdown_tx, shutdown_rx)
    }
}

pub struct ClientBuilder {
    shutdown_tx: Option<BSender<u8>>,
    shutdown_rx: Option<BReceiver<u8>>,
    pub fr: Option<FrRead>,
    pub fw: Option<FrWrite>,
    local_rx: Option<Receiver<Request>>,
    local_tx: Option<Sender<Request>>,
    io_id: u16,
}

#[async_trait]
impl Builder for ClientBuilder {
    type ClientType = Client;

    // Connect to remote server
    async fn setup_connection(&mut self, ct: ConnectType) -> io::Result<()> {
        if let ConnectType::ServerName(server) = ct {
            debug!("Client starting, connecting to server {:?}", server);

            let client = TcpStream::connect(server).await
                .map_err(|e| { error!("Unable to connect to server"); e })?;

            // split tcpstream so we can hand off to r & w tasks
            let (tcp_read, tcp_write) = client.into_split();

            self.fr = Some(FramedRead::new(tcp_read, ChatCodec));
            self.fw = Some(FramedWrite::new(tcp_write, ChatCodec));
        }

        Ok(())
    }

    // Setup channels support
    fn setup_channels(&mut self) {
        let (shutdown_tx, shutdown_rx) = broadcast::channel(16);
        self.shutdown_tx = Some(shutdown_tx);
        self.shutdown_rx = Some(shutdown_rx);

        let (local_tx, local_rx) = mpsc::channel::<Request>(64);
        self.local_tx = Some(local_tx);
        self.local_rx = Some(local_rx);
    }

    // Register new io id
    async fn setup_io(&mut self, io_shared: &InputShared, _eb: Option<&EventBus>) {
        self.io_id = io_shared.get_next_id().await;
    }

    // Build target structure (Client) from builder and move builder into target
    fn build(self) -> Client {
        Client::new(
            self.io_id,
            self
        )
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            shutdown_tx: None,
            shutdown_rx: None,
            fr: None,
            fw: None,
            local_rx: None,
            local_tx: None,
            io_id: u16::default(),
        }
    }

    // retrieval methods
    pub fn shutdown_handles(&mut self) -> (BSender<u8>, BReceiver<u8>) {
        (self.shutdown_tx.as_mut().unwrap().clone(), self.shutdown_tx.as_mut().unwrap().subscribe())
    }

    pub fn take_read(&mut self) -> FrRead {
        self.fr.take().unwrap()
    }

    pub fn take_write_fields(&mut self) -> (FrWrite, Receiver<Request>) {
        (self.fw.take().unwrap(), self.local_rx.take().unwrap())
    }

    pub fn take_tx(&mut self) -> Sender<Request> {
        self.local_tx.take().unwrap()
    }
}
