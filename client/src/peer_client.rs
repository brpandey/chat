use tokio::select;
use tokio::net::tcp;
use tokio::sync::{broadcast};
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio::sync::broadcast::Sender as BSender;
use tokio::sync::broadcast::Receiver as BReceiver;
use tokio_util::codec::{/*LinesCodec,*/ FramedRead, FramedWrite};
use tokio::net::TcpStream;
use tokio::io::{self}; //, empty, sink}; 
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait

use tracing::{info, debug, error};

use protocol::{Ask, ChatCodec, ChatMsg, Reply};
use crate::peer_types::PeerMsgType;
use crate::input_handler::{IO_ID_OFFSET, InputHandler, InputShared};

const SHUTDOWN: u8 = 1;
const PEER_HELLO: &str = "Peer {} is ready to chat";

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum PeerType {
    A,  // peer client node
    B, // peer server node
}

// Read and display peer server response
#[derive(Debug)]
pub struct PeerRead {
    tcp_read: Option<tcp::OwnedReadHalf>, // Some, use if type A
    client_rx: Option<Receiver<PeerMsgType>>, // Some, use if type B
    shutdown_tx: BSender<u8>,
}

impl PeerRead {
    pub fn new(tcp_read: Option<tcp::OwnedReadHalf>,
               client_rx: Option<Receiver<PeerMsgType>>,
               shutdown_tx: BSender<u8>
    ) -> Self {
        Self {
            tcp_read,
            client_rx,
            shutdown_tx
        }
    }

    // Loop to handle ongoing client msgs to server
    async fn handle_peer_read(&mut self, io_id: u16) {
        loop {
            debug!("task A: listening to peer server replies...");

            // Wrap handling of tcp_read which may be None
            let read_a = async {
                match &mut self.tcp_read {
                    Some(ref mut read) => {
                        let mut fr = FramedRead::new(read, ChatCodec);
                        Some(fr.next().await)
                    },
                    None => None,
                }
            };

            // Wrap handling of client_rx msg queue stream into another future to get
            // around weird tokio::select behaviour of still evaluating branch expression
            // even if conditional if expression is not true (e.g. and then calling unwrap on None)

            let read_b = async {
                match &mut self.client_rx {
                    Some(ref mut rx) => Some(rx.recv().await),
                    None => None,
                }
            };

            // In peer to peer mode, we have peer A and peer B,
            // peer A talks to peer B by sending a TCP message to B and B listens for messages
            // since peer A initiates the p2p session with peer B, it is the client in this instance
            // and B the peer server, though as a server B also can type cmd line messages!

            // select! read from server or if we are server (peer B), read from client_rx (peer A)

            // peer A (client only) --------------->   peer B (client and server) <---------- peer C (client only)
            select! {
                // Type B code
                // Display server received message and as peer B respond as fit
                // via command line
                  Some(msg_b) = read_b => {
                      //  Some(msg_b) = self.client_rx.as_mut().unwrap().recv(), if self.client_rx.is_some() => {
                    debug!("received X local channel peer server value is {:?}", msg_b);

                    match msg_b {
                        // if peer A wants to leave then terminate this peer 
                        Some(PeerMsgType::Leave(name)) => {
                            println!("< Session terminated as peer {} has left>", std::str::from_utf8(&name).unwrap_or_default());
                            self.shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                            return
                        },
                        Some(PeerMsgType::Hello(msg)) => {
                            println!("< {} >", std::str::from_utf8(&msg).unwrap_or_default());
                            println!("< To chat with peer, type: switch {} (peer type B)>", io_id - IO_ID_OFFSET);
                        },
                        Some(PeerMsgType::Note(msg)) => {
                            println!("P> {}", std::str::from_utf8(&msg).unwrap_or_default());
                        },
                        _ => unimplemented!(),
                    }
                }
                // Type A code
                // Read lines from server via tcp if we are peer A in the above scenario
                Some(msg_a) = read_a => {
                    debug!("received Y tcp peer server value is {:?}", msg_a);

                    match msg_a {
                        Some(Ok(ChatMsg::PeerB(Reply::Leave(msg)))) => { // peer B has left, terminate this peer
                            println!("< Session terminated as peer {} has left>", std::str::from_utf8(&msg).unwrap_or_default());
                            self.shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                            return
                        },
                        Some(Ok(ChatMsg::PeerB(Reply::Hello(name)))) => { // peer B has responded with hello
                            let name_str = std::str::from_utf8(&name).unwrap_or_default();
                            let hello_msg = PEER_HELLO.replace("{}", name_str).into_bytes();
                            println!("< {} >", std::str::from_utf8(&hello_msg).unwrap_or_default());
                            println!("< To chat with peer, type: switch {} (peer type A)>", io_id - IO_ID_OFFSET);
                        },
                        Some(Ok(ChatMsg::PeerB(Reply::Note(msg)))) => {
                            println!("P> {}", std::str::from_utf8(&msg).unwrap_or_default());
                        },
                        Some(Ok(_)) => unimplemented!(),
                        Some(Err(x)) => {
                            debug!("Peer Client Connection closing error: {:?}", x);
                            break;
                        },
                        None => break, //unimplemented!(),
                    }
                }
                else => {
                    info!("Peer Server Remote has closed");
                    break;
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct PeerWrite {
    tcp_write: Option<tcp::OwnedWriteHalf>,  // Some use if type A
    local_rx: Receiver<Ask>,
    server_tx: Option<Sender<PeerMsgType>>,  // Some use if type B
    shutdown_rx: BReceiver<u8>,
    client_type: PeerType,
}

impl PeerWrite {
    pub fn new(tcp_write: Option<tcp::OwnedWriteHalf>,
               local_rx: Receiver<Ask>, server_tx: Option<Sender<PeerMsgType>>,
               shutdown_rx: BReceiver<u8>,
               client_type: PeerType) -> Self {
        Self {
            tcp_write,
            local_rx,
            server_tx,
            shutdown_rx,
            client_type,
        }
    }

    async fn handle_peer_write(&mut self) {
        loop {
            select! {
                // Read from channel, data received from command line
                Some(msg) = self.local_rx.recv() => {
                    match self.client_type {
                        PeerType::A => { // send back to server across tcp
                            info!("handle_peer_write: peer type A {:?}", &msg);
                            let mut fw = FramedWrite::new(self.tcp_write.as_mut().unwrap(), ChatCodec);
                            fw.send(msg).await.expect("Unable to write to server");
                        },
                        PeerType::B => { // send back to local B server across msg channel
                            info!("handle_peer_write: peer type B {:?}", &msg);
                            match msg {
                                Ask::Note(m) => {
                                    self.server_tx.as_mut().unwrap().send(PeerMsgType::Note(m)).await.expect("Unable to tx");
                                },
                                Ask::Leave(m) => {
                                    self.server_tx.as_mut().unwrap().send(PeerMsgType::Leave(m)).await.expect("Unable to tx");
                                },
                                _ => unimplemented!(),
                            }
                        }
                    }
                }
                _ = self.shutdown_rx.recv() => { // exit task if any shutdown received
                    return
                }
            }
        }
    }
}

//#[derive(Debug)]
pub struct PeerClient {
    read: Option<PeerRead>,
    write: Option<PeerWrite>,
    local_tx: Option<Sender<Ask>>,
    shutdown_rx: Option<BReceiver<u8>>,
    name: String,
    io_id: u16,
}

impl PeerClient {
    pub async fn nospawn_a(server: String, name: String, io_shared: InputShared) -> u8 {
        if let Ok(mut client) = PeerClient::setup(Some(server), None, None, name, &io_shared, PeerType::A).await {
            // peer A initiates hello since it initiated the session!
            client.send_hello().await;
            client.run(io_shared).await;
        }

        42 // the meaning of life
    }

    /*
    pub fn spawn_a(server: String, name: String, io_shared: InputShared) {
        let _h = tokio::spawn(async move {
            if let Ok(mut client) = PeerClient::setup(Some(server), None, None, name, &io_shared, PeerType::A).await {
                // peer A initiates hello since it initiated the session!
                client.send_hello().await;
                client.run(io_shared).await;
            }
        });
    }
    */

    pub fn spawn_b(client_rx: Receiver<PeerMsgType>, server_tx: Sender<PeerMsgType>,
                   name: String, io_shared: InputShared) {
        let _h = tokio::spawn(async move {
            if let Ok(mut client) = PeerClient::setup(None, Some(client_rx), Some(server_tx), name, &io_shared, PeerType::B).await {
                client.run(io_shared).await;
            }
        });
    }

    pub async fn setup(server: Option<String>,
                       client_rx: Option<Receiver<PeerMsgType>>, server_tx: Option<Sender<PeerMsgType>>,
                       name: String,
                       io_shared: &InputShared,
                       client_type: PeerType)
                       -> io::Result<PeerClient> {

        // for communication between cmd line read and peer write
        let (local_tx, local_rx) = mpsc::channel::<Ask>(64);

        // for all peer client tasks shutdown (e.g. user has disconnected or peer user has disconnected)
        let (sd_tx, sd_rx1) = broadcast::channel(16);

        match client_type {
            // client is peer type A which initiates a reaquest to an already running peer B
            // client type A is not connected to the peer B server other than through tcp
            PeerType::A => {
                let client = TcpStream::connect(server.unwrap()).await
                    .map_err(|e| { error!("Unable to connect to server"); e })?;

                // split tcpstream so we can hand off to r & w tasks
                let (client_read, client_write) = client.into_split();

                let sd_rx2 = sd_tx.subscribe();

                let read = Some(PeerRead::new(Some(client_read), None, sd_tx));
                let write = Some(PeerWrite::new(Some(client_write), local_rx, None, sd_rx1, client_type));
                let io_id: u16 = io_shared.get_next_id().await;

                Ok(PeerClient {read, write, local_tx: Some(local_tx), shutdown_rx: Some(sd_rx2), name, io_id})
            },
            // client type B is the interative part of the peer type B server on the same node
            // client type B is connected to the peer type B through channels
            PeerType::B => {
                let sd_rx2 = sd_tx.subscribe();

                let read = Some(PeerRead::new(None, client_rx, sd_tx));
                let write = Some(PeerWrite::new(None, local_rx, server_tx, sd_rx1, client_type));
                let io_id: u16 = io_shared.get_next_id().await;

                Ok(PeerClient {read, write, local_tx: Some(local_tx), shutdown_rx: Some(sd_rx2), name, io_id})
            }
        }
    }

    async fn send_hello(&mut self) {
        let msg = Ask::Hello(self.name.clone().into_bytes());
        self.local_tx.as_mut().unwrap().send(msg).await.expect("xxx Unable to tx");
    }

    async fn run(&mut self, io_shared: InputShared) {
        let mut read = self.read.take().unwrap();
        let mut write = self.write.take().unwrap();
        let local_tx = self.local_tx.clone().unwrap();
        let mut shutdown_rx = self.shutdown_rx.take().unwrap();
        let io_id = self.io_id;
        let mut input_rx = io_shared.get_receiver();

        let peer_server_read_handle = tokio::spawn(async move {
            read.handle_peer_read(io_id).await;
        });

        let _peer_write_handle = tokio::spawn(async move {
            write.handle_peer_write().await;
        });

        let name = self.name.clone();
        // Use current thread to loop and grab data from command line
        let cmd_line_handle = tokio::spawn(async move {
            loop {
                debug!("task peer cmd line read: input looping");

                select! {
                    Ok(msg) = InputHandler::read_async_user_input(io_id, &mut input_rx, &io_shared) => {
                        let req = PeerClient::parse_input(&name, msg);
                        if req != Ask::Noop {
                            local_tx.send(req).await.expect("xxx Unable to tx");
                        }
                    }
                    _ = shutdown_rx.recv() => { // exit task if shutdown received
                        return
                    }
                }
            }
        });

        info!("gonzo A");
        peer_server_read_handle.await.unwrap();

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
