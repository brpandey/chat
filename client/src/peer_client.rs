use tokio::select;
use tokio::sync::{broadcast};
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio::sync::broadcast::Receiver as BReceiver;
use tokio_util::codec::{/*LinesCodec,*/ FramedRead, FramedWrite};
use tokio::net::TcpStream;
use tokio::io::{self}; //, empty, sink}; 

use tracing::{info, /*debug, */ error};

use protocol::{Ask, ChatCodec};
use crate::peer_types::PeerMsgType;
use crate::peer_reader_writer::{ReadHandle, WriteHandle, PeerReader, PeerWriter};
use crate::input_handler::{InputMsg, InputHandler, InputShared};

//#[derive(Debug)]
pub struct PeerClient {
    reader: Option<PeerReader>,
    writer: Option<PeerWriter>,
    local_tx: Option<Sender<Ask>>,
    shutdown_rx: Option<BReceiver<u8>>,
    name: String,
    io_id: u16,
}

impl PeerClient {
    pub async fn nospawn_a(server: String, name: String, peer_name: String, io_shared: InputShared) -> u8 {
        if let Ok(mut client) = PeerClient::setup_a(server, name, peer_name, &io_shared).await {
            // peer A initiates hello since it initiated the session!
            client.send_hello().await;
            client.run(io_shared).await;
        }

        42 // the meaning of life
    }

    pub fn spawn_b(client_rx: Receiver<PeerMsgType>, server_tx: Sender<PeerMsgType>,
                   name: String, io_shared: InputShared) {
        let _h = tokio::spawn(async move {
            if let Ok(mut client) = PeerClient::setup_b(client_rx, server_tx, name, &io_shared).await {
                client.run(io_shared).await;
            }
        });
    }

    // client is peer type A which initiates a reaquest to an already running peer B
    // client type A is not connected to the peer B server other than through tcp
    pub async fn setup_a(server: String,
                         name: String,
                         peer_name: String,
                         io_shared: &InputShared)
                         -> io::Result<PeerClient> {

        let client = TcpStream::connect(&server).await
            .map_err(|e| { error!("Unable to connect to server"); e })?;

        // split tcpstream so we can hand off to r & w tasks
        let (cl_read, cl_write) = client.into_split();

        // for communication between cmd line read and peer write
        let (local_tx, local_rx) = mpsc::channel::<Ask>(64);

        // for all peer client tasks shutdown (e.g. user has disconnected or peer user has disconnected)
        let (sd_tx, sd_rx1) = broadcast::channel(16);

        let sd_rx2 = sd_tx.subscribe();

        let rh = ReadHandle::A(FramedRead::new(cl_read, ChatCodec));
        let wh = WriteHandle::A(FramedWrite::new(cl_write, ChatCodec));
        let reader = Some(PeerReader::new(rh, sd_tx));
        let writer = Some(PeerWriter::new(wh, local_rx, sd_rx1));

        let io_id: u16 = io_shared.get_next_id().await;
        io_shared.notify(InputMsg::NewSession(io_id, peer_name.clone())).await.expect("Unable to send input msg");

        info!("New peer client A, name: {} io_id: {}, successful tcp connect to peer server {:?}", &name, io_id, &server);

        Ok(PeerClient {reader, writer, local_tx: Some(local_tx), shutdown_rx: Some(sd_rx2), name, io_id})
    }


    // client type B is the interative part of the peer type B server on the same node
    // client type B is connected to the peer type B through channels
    pub async fn setup_b(client_rx: Receiver<PeerMsgType>,
                         server_tx: Sender<PeerMsgType>,
                         name: String,
                         io_shared: &InputShared)
                         -> io::Result<PeerClient> {

        // for communication between cmd line read and peer write
        let (local_tx, local_rx) = mpsc::channel::<Ask>(64);

        // for all peer client tasks shutdown (e.g. user has disconnected or peer user has disconnected)
        let (sd_tx, sd_rx1) = broadcast::channel(16);

        let sd_rx2 = sd_tx.subscribe();

        let rh = ReadHandle::B(client_rx);
        let wh = WriteHandle::B(server_tx);
        let reader = Some(PeerReader::new(rh, sd_tx));
        let writer = Some(PeerWriter::new(wh, local_rx, sd_rx1));

        let io_id: u16 = io_shared.get_next_id().await;

        info!("New peer client B, name: {} io_id: {}, set up local channel to local peer server", &name, io_id);

        Ok(PeerClient {reader, writer, local_tx: Some(local_tx), shutdown_rx: Some(sd_rx2), name, io_id})
    }

    async fn send_hello(&mut self) {
        let msg = Ask::Hello(self.name.clone().into_bytes());
        self.local_tx.as_mut().unwrap().send(msg).await.expect("xxx Unable to tx");
    }

    async fn run(&mut self, io_shared: InputShared) {
        let mut reader = self.reader.take().unwrap();
        let mut writer = self.writer.take().unwrap();
        let local_tx = self.local_tx.clone().unwrap();
        let mut shutdown_rx = self.shutdown_rx.take().unwrap();
        let io_id = self.io_id;
        let mut input_rx = io_shared.get_receiver();
        let io_notify1 = io_shared.get_notifier();
        let io_notify2 = io_shared.get_notifier();

        let peer_server_read_handle = tokio::spawn(async move {
            reader.handle_peer_read(io_id, io_notify1).await;
        });

        let _peer_write_handle = tokio::spawn(async move {
            writer.handle_peer_write().await;
        });

        let name = self.name.clone();
        // Use current thread to loop and grab data from command line
        let cmd_line_handle = tokio::spawn(async move {
            loop {
                info!("task peer cmd line read: input looping");

                select! {
                    Ok(msg) = InputHandler::read_async_user_input(io_id, &mut input_rx, &io_shared) => {
                        let req = PeerClient::parse_input(&name, msg);
                        if req != Ask::Noop {
                            info!("about to send peer client cmd line input request data on local_tx");
                            local_tx.send(req).await.expect("xxx Unable to tx");
                        } else {
                            info!("noop hence no local tx send");
                        }
                    }
                    _ = shutdown_rx.recv() => { // exit task if shutdown received
                        info!("shutdown received!");
                        return
                    }
                }
            }
        });

        info!("gonzo A");
        peer_server_read_handle.await.unwrap();

        // given read task is finished (e.g. through \leave or disconnect) switch back to lobby session
        io_notify2.send(InputMsg::CloseSession(io_id)).await.expect("Unable to send close sesion msg");

        info!("gonzo B");

        cmd_line_handle.await.unwrap();

        info!("gonzo C");
    }

    pub fn parse_input(name: &str, line: Option<String>) -> Ask {
        if line.is_none() { return Ask::Noop }
        match line.unwrap().as_str() {
            "\\leave" => {
                info!("Private session ended by user {}", name);
                return Ask::Leave(name.as_bytes().to_vec());
            },
            l => {
                info!("not a peerclient command message");
                // if no commands, split up user input
                let mut out = vec![];
                out.push(format!("<{}> ", name).into_bytes());
                let msg = InputHandler::interleave_newlines(l, out);
                return Ask::Note(msg)
            },
        }
    }
}
