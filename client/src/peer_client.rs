use tokio::select;
use tokio::net::tcp;
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio_util::codec::{LinesCodec, FramedRead, FramedWrite};
use tokio::net::TcpStream;
use tokio::io::{self}; //, empty, sink}; 
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait

use tracing::{info, debug, error};

use protocol::{Ask, ChatCodec, ChatMsg, Reply};

use crate::peer_types::PeerMsgType;

const LINES_MAX_LEN: usize = 256;
const USER_LINES: usize = 64;

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
}

impl PeerRead {
    pub fn new(tcp_read: Option<tcp::OwnedReadHalf>,
               client_rx: Option<Receiver<PeerMsgType>>) -> Self {
        Self {
            tcp_read,
            client_rx,
        }
    }

    // Loop to handle ongoing client msgs to server
    async fn handle_peer_read(&mut self) {

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

            //  peer A (client only) --------------->   peer B (client and server) <---------- peer C (client only)
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
                            println!("< {} >", std::str::from_utf8(&name).unwrap());
                            return
                        },
                        Some(PeerMsgType::Hello(msg)) => {
                            println!("< {} >", std::str::from_utf8(&msg).unwrap());
                        },
                        Some(PeerMsgType::Note(msg)) => {
                            println!("P> {}", std::str::from_utf8(&msg).unwrap());
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
                            println!("< {} >", std::str::from_utf8(&msg).unwrap());
                            return
                        },
                        Some(Ok(ChatMsg::PeerB(Reply::Hello(msg)))) => { // peer B has responded with hello
                            println!("< {} >", std::str::from_utf8(&msg).unwrap());
                        },
                        Some(Ok(ChatMsg::PeerB(Reply::Note(msg)))) => {
                            println!("P> {}", std::str::from_utf8(&msg).unwrap());
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
    client_type: PeerType,
}

impl PeerWrite {
    pub fn new(tcp_write: Option<tcp::OwnedWriteHalf>,
               local_rx: Receiver<Ask>, server_tx: Option<Sender<PeerMsgType>>,
               client_type: PeerType) -> Self {
        Self {
            tcp_write,
            local_rx,
            server_tx,
            client_type,
        }
    }

    async fn handle_peer_write(&mut self) {
        loop {
            // Read from channel, data received from command line
            if let Some(msg) = self.local_rx.recv().await {
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
            } else {
                info!("peer write terminating, no more channel senders");
                break;
            }
        }
    }
}

//#[derive(Debug)]
pub struct PeerClient {
    read: Option<PeerRead>,
    write: Option<PeerWrite>,
    local_tx: Option<Sender<Ask>>,
    name: String,
}

impl PeerClient {
    pub fn spawn_a(server: String, name: String) {
        let _h = tokio::spawn(async move {
            if let Ok(mut client) = PeerClient::setup(Some(server), None, None, name, PeerType::A).await {
                client.run().await;
            }
        });
    }

    pub fn spawn_b(client_rx: Receiver<PeerMsgType>, server_tx: Sender<PeerMsgType>, name: String) {
        let _h = tokio::spawn(async move {
            if let Ok(mut client) = PeerClient::setup(None, Some(client_rx), Some(server_tx), name, PeerType::B).await {
                client.run().await;
            }
        });
    }

    pub async fn setup(server: Option<String>,
                       client_rx: Option<Receiver<PeerMsgType>>, server_tx: Option<Sender<PeerMsgType>>,
                       name: String,
                       client_type: PeerType) -> io::Result<PeerClient> {

        let (local_tx, local_rx) = mpsc::channel::<Ask>(64);

        match client_type {
            // client is peer type A which initiates a reaquest to an already running peer B
            PeerType::A => {
                let client = TcpStream::connect(server.unwrap()).await
                    .map_err(|e| { error!("Unable to connect to server"); e })?;

                // split tcpstream so we can hand off to r & w tasks
                let (client_read, client_write) = client.into_split();

                let read = Some(PeerRead::new(Some(client_read), None));
                let write = Some(PeerWrite::new(Some(client_write), local_rx, None, client_type));

                Ok(PeerClient {read, write, local_tx: Some(local_tx), name}) //, client_type})
            },

            // client is peer type B coupled with running peer type B server on the same node
            PeerType::B => {
                let read = Some(PeerRead::new(None, client_rx));
                let write = Some(PeerWrite::new(None, local_rx, server_tx, client_type));

                Ok(PeerClient {read, write, local_tx: Some(local_tx), name}) //, client_type})
            }
        }
    }

    pub async fn run(&mut self) {
        let mut read = self.read.take().unwrap();
        let mut write = self.write.take().unwrap();
        let local_tx = self.local_tx.take().unwrap();

        let peer_server_read_handle = tokio::spawn(async move {
            read.handle_peer_read().await;
        });

        let _peer_write_handle = tokio::spawn(async move {
            write.handle_peer_write().await;
        });

        let name = self.name.clone();
        // Use current thread to loop and grab data from command line
        let _cmd_line_handle = tokio::spawn(async move {
            loop {
                debug!("task peer cmd line read: input looping");

                if let Some(msg) = read_async_user_input(&name).await {
                    local_tx.send(msg).await.expect("xxx Unable to tx");
                } else {
                    break;
                }
            }
        });

        peer_server_read_handle.await.unwrap();
    }
}

async fn read_async_user_input(name: &str) -> Option<Ask> {
    let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new_with_max_length(LINES_MAX_LEN));

    // need to implement some sort of lock on stdin
    // to coordinate between data intended for rendezvous server or
    // even input destined for another peer

    if let Some(Ok(line)) = fr.next().await {
        // handle user input-ed commands
        match line.as_str() {
            "\\leave" => {
                info!("Private session ended by user {}", name);
                return Some(Ask::Leave(name.as_bytes().to_vec()));
            },
            _ => (),
        }

        // if no commands, split up user input
        let mut total = vec![];

        let msg_prefix = format!("<{}> ", name);
        total.push(msg_prefix.into_bytes());
        let mut citer = line.as_bytes().chunks(USER_LINES);

        while let Some(c) = citer.next() {
            let mut line = c.to_vec();
            line.push(b'\n');
            total.push(line);
        }

        let msg: Vec<u8> = total.into_iter().flatten().collect();
        return Some(Ask::Note(msg))
    }

    None
}
