use tokio::io;
use tokio::select;
use tokio::sync::mpsc::{Sender, Receiver};

use protocol::Ask;
use crate::builder::PeerClientBuilder as Builder;
use crate::peer_types::PeerMsgType;
use crate::input_handler::{InputMsg, InputHandler};
use crate::input_shared::InputShared;

use tracing::{info, /*debug, error */};

const SHUTDOWN: u8 = 1;

//#[derive(Debug)]
pub struct PeerClient {
    name: String,
    io_id: u16,
    local_tx: Option<Sender<Ask>>,
    builder: Builder,
}

impl PeerClient {
    pub fn new(name: String, io_id: u16, local_tx: Option<Sender<Ask>>, builder: Builder) -> Self {
        Self {
            name,
            io_id,
            local_tx,
            builder,
        }
    }

    pub async fn nospawn_a(server: String, name: String, peer_name: String, io_shared: InputShared) -> () {
        if let Ok(mut client) = PeerClient::build_a(server, name, peer_name, &io_shared).await {
            // peer A initiates hello since it initiated the session!
            client.send_hello().await;
            client.run(io_shared).await;
        }

        ()
    }

    pub async fn nospawn_b(client_rx: Receiver<PeerMsgType>, server_tx: Sender<PeerMsgType>,
                   name: String, io_shared: InputShared) -> () {
        if let Ok(mut client) = PeerClient::build_b(client_rx, server_tx, name, &io_shared).await {
            client.run(io_shared).await;
        }

        ()
    }

    // client is peer type A which initiates a reaquest to an already running peer B
    // client type A is not connected to the peer B server other than through tcp
    pub async fn build_a(server: String,
                           name: String,
                           peer_name: String,
                           io_shared: &InputShared) -> io::Result<PeerClient> {

        let client = Builder::new(name)
            .connect(&server).await?
            .channels()
            .io_register_notify(io_shared, peer_name).await
            .build();

        info!("New peer client A, name: {} io_id: {}, successful tcp connect to peer server {:?}",
              &client.name, client.io_id, &server);

        Ok(client)
    }

    // client type B is the interative part of the peer type B server on the same node
    // client type B is connected to the peer type B through channels
    pub async fn build_b(client_rx: Receiver<PeerMsgType>,
                         server_tx: Sender<PeerMsgType>,
                         name: String,
                         io_shared: &InputShared)
                         -> io::Result<PeerClient> {

        let client = Builder::new(name)
            .connect_local(client_rx, server_tx)
            .channels()
            .io_register(io_shared).await
            .build();

        info!("New peer client B, name: {} io_id: {}, set up local channel to local peer server",
              &client.name, client.io_id);

        Ok(client)
    }

    async fn send_hello(&mut self) {
        let msg = Ask::Hello(self.name.clone().into_bytes());
        self.local_tx.as_mut().unwrap().send(msg).await.expect("xxx Unable to tx");
    }

    async fn run(&mut self, io_shared: InputShared) {
        // grab builder fields
        let (mut reader, mut writer, shutdown_tx, mut shutdown_rx) = self.builder.take_fields();

        // grab self field data
        let name = self.name.clone();
        let io_id = self.io_id;
        let local_tx = self.local_tx.clone().unwrap();

        // grab input handler related data
        let mut input_rx = io_shared.get_receiver();
        let (io_notify1, io_notify2) = (io_shared.get_notifier(), io_shared.get_notifier());

        let peer_server_read_handle = tokio::spawn(async move {
            reader.handle_peer_read(io_id, io_notify1).await;
        });

        let _peer_write_handle = tokio::spawn(async move {
            writer.handle_peer_write().await;
        });

        // Use current thread to loop and grab data from command line
        let cmd_line_handle = tokio::spawn(async move {
            loop {
                info!("task peer cmd line read: input looping");

                select! {
                    input = async {
                        let req = InputHandler::read_async_user_input(io_id, &mut input_rx, &io_shared).await?
                            .and_then(|m| PeerClient::parse_input(&name, m));

                        if req.is_some() {
                            local_tx.send(req.unwrap()).await.expect("xxx Unable to tx");
                        }

                        Ok::<_, io::Error>(())
                    } => {
                        if input.is_err() { // if input handler has received a terminate
                            info!("sending shutdown msg C");
                            shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                            return
                        }
                    }
                    _ = shutdown_rx.recv() => { // exit task if shutdown received
                        info!("shutdown received!");
                        return
                    }
                }
            }
        });

        peer_server_read_handle.await.unwrap();

        // given read task is finished (e.g. through \leave or disconnect) switch back to lobby session
        io_notify2.send(InputMsg::CloseSession(io_id)).await.expect("Unable to send close sesion msg");

        cmd_line_handle.await.unwrap();

        info!("Peer client exiting!");
    }

    pub fn parse_input(name: &str, line: String) -> Option<Ask> {
        match line.as_str() {
            "\\leave" | "\\quit" => {
                info!("Private session ended by user {}", name);
                return Some(Ask::Leave(name.as_bytes().to_vec()));
            },
            l => {
                info!("not a peerclient command message");
                // if no commands, split up user input
                let mut out = vec![];
                out.push(format!("<{}> ", name).into_bytes());
                let msg = InputHandler::interleave_newlines(l, out);
                return Some(Ask::Note(msg))
            },
        }
    }
}
