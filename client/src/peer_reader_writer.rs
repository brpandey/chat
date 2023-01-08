use tokio::select;
use tokio::net::tcp;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::sync::broadcast::Sender as BSender;
use tokio::sync::broadcast::Receiver as BReceiver;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait

use tracing::{info, debug};

use protocol::{Ask, ChatCodec, ChatMsg, Reply};
use crate::peer_types::PeerMsgType;
use crate::input_handler::{IO_ID_OFFSET, InputNotifier, InputMsg};

type FrRead = FramedRead<tcp::OwnedReadHalf, ChatCodec>;
type FrWrite = FramedWrite<tcp::OwnedWriteHalf, ChatCodec>;
type ShutdownTx = BSender<u8>;

const SHUTDOWN: u8 = 1;
const PEER_HELLO: &str = "Peer {} is ready to chat";

//  A is peer client type, B is peer server type
#[derive(Debug)]
pub enum ReadHandle {
    A(FrRead), // read from tcp connection
    B(Receiver<PeerMsgType>), // read from local channel from local server
}

#[derive(Debug)]
pub enum WriteHandle {
    A(FrWrite), // write back using tcp write
    B(Sender<PeerMsgType>), // write to local channel to local server
}

// Read and display peer server response
#[derive(Debug)]
pub struct PeerReader {
    read: ReadHandle,
    kill: ShutdownTx,
}

impl PeerReader {
    pub fn new(read: ReadHandle, kill: ShutdownTx) -> Self {
        Self {
            read,
            kill,
        }
    }

    // Loop to handle ongoing client msgs to server
    pub async fn handle_peer_read(&mut self, io_id: u16, io_notify: InputNotifier) {
        // In peer to peer mode, we have peer A and peer B,
        // peer A talks to peer B by sending a TCP message to B and B listens for messages
        // since peer A initiates the p2p session with peer B, it is the client in this instance
        // and B the peer server, though as a server B also can type cmd line messages!

        // select! read from server or if we are server (peer B), read from client_rx (peer A)

        // peer A (client only) --------------->   peer B (client and server) <---------- peer C (client only)
        let mut br;
        loop {
            debug!("task A: listening to peer server replies...");

            match &mut self.read {
                ReadHandle::A(ref mut fr) => br = Self::peer_read_a(&mut self.kill, fr).await,
                ReadHandle::B(ref mut client_rx) => br = Self::peer_read_b(&mut self.kill, client_rx, io_id, &io_notify).await,
            }

            if br { break; }
        }
    }

    async fn peer_read_a(kill: &mut ShutdownTx, fr: &mut FrRead) -> bool {
        // Type A code
        // Read lines from server via tcp if we are peer A in the above scenario

        let mut br = false; // break value

        select! {
            Some(msg_a) = fr.next() => {
                info!("received Y tcp peer server value is {:?}", msg_a);

                match msg_a {
                    Ok(ChatMsg::PeerB(Reply::Leave(msg))) => { // peer B has left, terminate this peer
                        println!("< Session terminated as peer {} has left>", std::str::from_utf8(&msg).unwrap_or_default());
                        kill.send(SHUTDOWN).expect("Unable to send shutdown");
                        br = true;
                    },
                    Ok(ChatMsg::PeerB(Reply::Hello(name))) => { // peer B has responded with hello
                        let name_str = std::str::from_utf8(&name).unwrap_or_default();
                        let hello_msg = PEER_HELLO.replace("{}", &name_str).into_bytes();
                        println!("< {} >", std::str::from_utf8(&hello_msg).unwrap_or_default());
                    },
                    Ok(ChatMsg::PeerB(Reply::Note(msg))) => {
                        println!("P> {}", std::str::from_utf8(&msg).unwrap_or_default());
                    },
                    Ok(_) => unimplemented!(),
                    Err(x) => {
                        debug!("Peer Client Connection closing error: {:?}", x);
                        br = true;
                    },
                }
            }
            else => {
                info!("Peer Server Remote has closed");
                br = true;
            }
        }

        br
    }

    async fn peer_read_b(kill: &mut ShutdownTx, client_rx: &mut Receiver<PeerMsgType>,
                         io_id: u16, io_notify: &InputNotifier) -> bool {
        // Type B code
        // Display server received message and as peer B respond as fit
        // via command line
        let mut br = false;

        select! {
            Some(msg_b) = client_rx.recv() => {
                //  Some(msg_b) = self.client_rx.as_mut().unwrap().recv(), if self.client_rx.is_some() => {
                info!("received X local channel peer server value is {:?}", msg_b);

                match msg_b {
                    // if peer A wants to leave then terminate this peer 
                    PeerMsgType::Leave(name) => {
                        println!("< Session terminated as peer {} has left>", std::str::from_utf8(&name).unwrap_or_default());
                        kill.send(SHUTDOWN).expect("Unable to send shutdown");
                        br = true;
                    },
                    PeerMsgType::Hello(name, msg) => {
                        // since peer type b is create after tcp request name is not provided initially
                        // hence record name given hello protocol msg
                        io_notify.send(InputMsg::UpdatedSessionName(io_id, name))
                            .await.expect("Unable to send close sesion msg");

                        println!("< {}, to chat, type: \\s {} (peer type B), to return to lobby, type: \\s 0 >",
                                 std::str::from_utf8(&msg).unwrap_or_default(), io_id - IO_ID_OFFSET);
                    },
                    PeerMsgType::Note(msg) => {
                        println!("P> {}", std::str::from_utf8(&msg).unwrap_or_default());
                    },
                }
            }

            else => {
                info!("No client transmitters, server must have dropped");
                br = true;
            }
        }

        br
    }
}

#[derive(Debug)]
pub struct PeerWriter {
    write: WriteHandle,
    local_rx: Receiver<Ask>,
    shutdown_rx: BReceiver<u8>,
}

impl PeerWriter {
    pub fn new(write: WriteHandle,
               local_rx: Receiver<Ask>,
               shutdown_rx: BReceiver<u8>)
               -> Self {
        Self {
            write,
            local_rx,
            shutdown_rx,
        }
    }

    pub async fn handle_peer_write(&mut self) {
        loop {
            select! {
                // Read from channel, data received from command line
                Some(msg) = self.local_rx.recv() => {
                    match &mut self.write {
                        WriteHandle::A(ref mut fw) => {
                            info!("handle_peer_write: peer type A {:?}", &msg);
                            fw.send(msg).await.expect("Unable to write to tcp server");
                        },
                        WriteHandle::B(ref server_tx) => {
                            info!("handle_peer_write: peer type B {:?}", &msg);
                            match msg {
                                Ask::Note(m) => {
                                    server_tx.send(PeerMsgType::Note(m)).await.expect("Unable to tx");
                                },
                                Ask::Leave(m) => {
                                    server_tx.send(PeerMsgType::Leave(m)).await.expect("Unable to tx");
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
