use tokio::select;
use tokio::net::tcp;
use tokio::sync::mpsc::{Receiver};
use tokio::sync::broadcast::{Sender as BSender, Receiver as BReceiver};
use tokio_util::codec::{FramedRead};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait

use tracing::{info, debug, error};

use protocol::{ChatCodec, ChatMsg, Reply};
use crate::types::{PeerMsg, InputMsg};
use crate::input_shared::InputNotifier;
use crate::input_reader::session_id;
use crate::peer_set::PeerShared;

type FrRead = FramedRead<tcp::OwnedReadHalf, ChatCodec>;
type ShutdownTx = BSender<u8>;
type ShutdownRx = BReceiver<u8>;

const SHUTDOWN: u8 = 1;
const PEER_HELLO: &str = "Peer {} is ready to chat";

//  A is peer client type, B is peer server type
#[derive(Debug)]
pub enum ReadHandle {
    A(FrRead), // read from tcp connection
    B(Receiver<PeerMsg>), // read from local channel from local server
}

// Read and display peer server response
#[derive(Debug)]
pub struct PeerReader {
    read: ReadHandle,
    kill: ShutdownTx,
    kill_rx: ShutdownRx,
}

impl PeerReader {
    pub fn new(read: ReadHandle, kill: ShutdownTx, kill_rx: ShutdownRx) -> Self {
        Self {
            read,
            kill,
            kill_rx,
        }
    }

    // Loop to handle ongoing client msgs to server
    pub async fn handle_peer_read(&mut self, io_id: u16, io_notify: InputNotifier, peer_shared: PeerShared) -> Option<String> {
        // In peer to peer mode, we have peer A and peer B,
        // peer A talks to peer B by sending a TCP message to B and B listens for messages
        // since peer A initiates the p2p session with peer B, it is the client in this instance
        // and B the peer server, though as a server B also can type cmd line messages!

        // select! read from server or if we are server (peer B), read from client_rx (peer A)

        // peer A (client only) --------------->   peer B (client and server) <---------- peer C (client only)
        let mut br;
        let mut peer_name: Option<String> = None;

        loop {
            debug!("task A: listening to peer server replies...");

            match &mut self.read {
                ReadHandle::A(ref mut fr) => br = Self::peer_read_a(&mut self.kill, &mut self.kill_rx, fr).await,
                ReadHandle::B(ref mut client_rx) => br = Self::peer_read_b(&mut self.kill, &mut self.kill_rx,
                                                                           client_rx, io_id, &io_notify, &peer_shared, &mut peer_name).await,
            }

            if br { break; }
        }

        peer_name
    }


    async fn peer_read_a(kill: &mut ShutdownTx, kill_rx: &mut ShutdownRx, fr: &mut FrRead) -> bool {
        // Type A code
        // Read lines from server via tcp if we are peer A in the above scenario

        let mut br = false; // break value

        select! {
            peer_data = async {
                let msg_a = fr.next().await?;
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

                Some(())
            } => {
                if peer_data.is_none() { // if input handler has received a terminate
                    info!("Peer Server Remote has closed!");
                    if kill.send(SHUTDOWN).is_err() {
                        error!("Unable to send shutdown");
                    }
                    br = true;
                }
            }
            _ = kill_rx.recv() => { // exit task if any shutdown received
                br = true;
            }
        }

        br
    }

    async fn peer_read_b(kill: &mut ShutdownTx, kill_rx: &mut ShutdownRx, client_rx: &mut Receiver<PeerMsg>,
                         io_id: u16, io_notify: &InputNotifier, peer_shared: &PeerShared, peer_name: &mut Option<String>) -> bool {
        // Type B code
        // Display server received message and as peer B respond as fit
        // via command line
        let mut br = false;

        select! {
            Some(msg_b) = client_rx.recv() => {
                info!("received X local channel peer server value is {:?}", msg_b);

                match msg_b {
                    // if peer A wants to leave then terminate this peer 
                    PeerMsg::Leave(name) => {
                        println!("< Session terminated as peer {} has left>", std::str::from_utf8(&name).unwrap_or_default());
                        kill.send(SHUTDOWN).expect("Unable to send shutdown");
                        br = true;
                    },
                    PeerMsg::Hello(name, msg) => {
                        // since peer type b is create after tcp request name is not provided initially
                        // hence record name given hello protocol msg
                        io_notify.send(InputMsg::UpdatedSessionName(io_id, name.clone()))
                            .await.expect("Unable to send close sesion msg");

                        peer_shared.insert(name.clone()).await;

                        *peer_name = Some(name);

                        println!("< {}, to chat, type: \\sw {} (peer type B), to return to lobby, type: \\sw 0 >",
                                 std::str::from_utf8(&msg).unwrap_or_default(), session_id(io_id));
                    },
                    PeerMsg::Note(msg) => {
                        println!("P> {}", std::str::from_utf8(&msg).unwrap_or_default());
                    },
                }
            }
            _ = kill_rx.recv() => { // exit task if any shutdown received
                br = true;
            }
        }

        br
    }
}