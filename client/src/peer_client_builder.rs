use tokio::io;
use tokio::sync::{broadcast};
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio::sync::broadcast::Receiver as BReceiver;
use tokio::sync::broadcast::Sender as BSender;
use tokio_util::codec::{/*LinesCodec,*/ FramedRead, FramedWrite};
use tokio::net::TcpStream;

use protocol::{Ask, ChatCodec};
use crate::peer_client::PeerClient;
use crate::peer_types::PeerMsgType;
use crate::peer_reader_writer::{ReadHandle, WriteHandle, PeerReader, PeerWriter};
use crate::input_handler::{InputMsg, InputShared};

use tracing::error;

pub struct Builder {
    name: Option<String>,
    rh: Option<ReadHandle>,
    wh: Option<WriteHandle>,
    reader: Option<PeerReader>,
    writer: Option<PeerWriter>,
    shutdown_tx: Option<BSender<u8>>,
    shutdown_rx: Option<BReceiver<u8>>,
    local_tx: Option<Sender<Ask>>,
    io_id: u16,
}

// Builder provides a simpler, reusable and pipelined interface when constructing Peer Client structure
impl Builder {
    pub fn new(name: String) -> Self {
        Self {
            name: Some(name),
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

    // Used for PeerClient type A
    pub async fn connect(mut self, server: &str) -> io::Result<Self> {
        let client = TcpStream::connect(server).await
            .map_err(|e| { error!("Unable to connect to server"); e })?;
        // split tcpstream so we can hand off to r & w tasks
        let (tcp_read, tcp_write) = client.into_split();

        // stash r+w handles
        self.rh = Some(ReadHandle::A(FramedRead::new(tcp_read, ChatCodec)));
        self.wh = Some(WriteHandle::A(FramedWrite::new(tcp_write, ChatCodec)));

        Ok(self)
    }

    // Used for PeerClient type B
    pub fn connect_local(mut self, client_rx: Receiver<PeerMsgType>, server_tx: Sender<PeerMsgType>) -> Self {
        // stash r+w handles
        self.rh = Some(ReadHandle::B(client_rx));
        self.wh = Some(WriteHandle::B(server_tx));
        self
    }

    // Setup channels support
    pub fn channels(mut self) -> Self {
        // 1) communication between cmd line read and peer write
        // 2) for all peer client tasks shutdown (e.g. user has disconnected or peer user has disconnected)
        let (local_tx, local_rx) = mpsc::channel::<Ask>(64); // 1
        let (sd_tx, sd_rx) = broadcast::channel(16); // 2
        self.local_tx = Some(local_tx);

        // stash peer reader and writers
        self.reader = Some(PeerReader::new(self.rh.take().unwrap(), sd_tx.clone()));
        self.writer = Some(PeerWriter::new(self.wh.take().unwrap(), local_rx, sd_rx));

        // stash shutdown channel handles
        self.shutdown_rx = Some(sd_tx.subscribe());
        self.shutdown_tx = Some(sd_tx);

        self
    }

    // Register new io id and notify
    pub async fn io_register_notify(mut self, io_shared: &InputShared, peer_name: &str) -> Self {
        self.io_id = io_shared.get_next_id().await;
        io_shared.notify(InputMsg::NewSession(self.io_id, peer_name.to_owned())).await.expect("Unable to send input msg");
        self
    }

    // Register new io id 
    pub async fn io_register(mut self, io_shared: &InputShared) -> Self {
        self.io_id = io_shared.get_next_id().await;
        self
    }

    // Build target structure (PeerClient) from builder and moving builder into target
    pub fn build(mut self) -> PeerClient {
        PeerClient::new(
            self.name.take().unwrap(),
            self.io_id,
            self.local_tx.take(),
            self
        )
    }

    // Extracts relevant fields into single tuple
    pub fn take_fields(&mut self) -> (PeerReader, PeerWriter, BSender<u8>, BReceiver<u8>) {
        let reader = self.reader.take().unwrap();
        let writer = self.writer.take().unwrap();
        let shutdown_tx = self.shutdown_tx.take().unwrap();
        let shutdown_rx = self.shutdown_rx.take().unwrap();

        (reader, writer, shutdown_tx, shutdown_rx)
    }
}



