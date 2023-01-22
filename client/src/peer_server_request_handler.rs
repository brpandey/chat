use tokio::select;
use tokio::net::tcp;
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_stream::StreamExt;

use futures::SinkExt;
use tracing::{debug};

use crate::types::PeerMsg;
use protocol::{Ask, ChatCodec, ChatMsg, Reply};

const PEER_LEFT: &str = "Peer {} has left";
const PEER_HELLO: &str = "Peer {} is ready to chat";

// Just a reader and writer (no need to handle cmd line as peer client b does this)
pub struct PeerServerRequestHandler {
    reader: Option<PeerServerReader>,
    writer: Option<PeerServerWriter>,
}

struct PeerServerReader {
    tcp_read: Option<tcp::OwnedReadHalf>,
    local_tx: Sender<PeerMsg>,
    peer_b_client_tx: Sender<PeerMsg>,
    name: String,
}

struct PeerServerWriter {
    tcp_write: Option<tcp::OwnedWriteHalf>,
    local_rx: Receiver<PeerMsg>,
    peer_b_server_rx: Receiver<PeerMsg>,
}

const BOUNDED_CHANNEL_SIZE: usize = 64;

impl PeerServerRequestHandler {
    pub fn new(tcp_read: tcp::OwnedReadHalf, tcp_write: tcp::OwnedWriteHalf,
               peer_b_client_tx: Sender<PeerMsg>,
               peer_b_server_rx: Receiver<PeerMsg>,
               name: String
    ) -> Self {

        // Setup server local msg passing channel
        let (local_tx, local_rx) = mpsc::channel::<PeerMsg>(BOUNDED_CHANNEL_SIZE);

        Self {
            reader: Some(PeerServerReader {
                tcp_read: Some(tcp_read),
                local_tx,
                peer_b_client_tx,
                name,
            }),
            writer: Some(PeerServerWriter {
                tcp_write: Some(tcp_write),
                local_rx,
                peer_b_server_rx,
            })
        }
    }

    pub fn spawn(&mut self) {
        let mut r = self.reader.take().unwrap();
        let mut w = self.writer.take().unwrap();

        // Spawn tokio task to handle server socket reads from clients
        let _read = tokio::spawn(async move {
            r.handle_read().await;
        });

        // Spawn tokio task to handle server socket writes to clients
        let _write = tokio::spawn(async move {
            w.handle_write().await;
        });
    }
}

impl PeerServerReader {
    // Loop to handle incoming client msgs to server
    async fn handle_read(&mut self) {
        let input = self.tcp_read.take().unwrap();
        let mut fr = FramedRead::new(input, ChatCodec);

        loop {
            if let Some(value) = fr.next().await {
                debug!("Peer server received from tcp_read: {:?}", value);

                match value {
                    Ok(ChatMsg::PeerA(Ask::Hello(name))) => {
                        let name_str = String::from_utf8(name).unwrap_or_default();
                        let hello_msg = PEER_HELLO.replace("{}", &name_str).into_bytes();
                        // send msg to this local peer b node
                        self.peer_b_client_tx.send(PeerMsg::Hello(name_str, hello_msg)).await
                                            .expect("Unable to tx");
                        // send response msg back to peer a with peer b's name
                        self.local_tx.send(PeerMsg::Hello(String::new(),
                                                              self.name.clone().into_bytes())).await // send back peer b server's name
                            .expect("Unable to tx");
                    },
                    Ok(ChatMsg::PeerA(Ask::Leave(name))) => {
                        self.process_disconnect(name.clone()).await;

                        // send local msg
                        self.local_tx.send(PeerMsg::Leave(vec![])).await // send back peer b server's name
                            .expect("Unable to tx");

                        break;
                    },
                    Ok(ChatMsg::PeerA(Ask::Note(msg))) => {
                        for line in msg.split(|e| *e == b'\n').map(|l| l.to_owned()) {
                            if line.is_empty() { continue }
                            self.peer_b_client_tx.send(PeerMsg::Note(line)).await
                                .expect("Unable to tx");
                        }
                    },
                    Ok(_) => unimplemented!(),
                    Err(x) => {
                        debug!("Server Connection closing error: {:?}", x);
                        break;
                    },
                }
            } else {
                self.process_disconnect("current".as_bytes().to_vec()).await;
//                self.process_disconnect(self.name.clone().into_bytes()).await;
                break;
            }
        }
    }

    // process client disconnection event
    async fn process_disconnect(&mut self, name: Vec<u8>) {
        let name_str = std::str::from_utf8(&name).unwrap_or_default();
        debug!("Process Disconnect - Client connection connected to {:?} has closed", &name_str);
        let leave_msg = PEER_LEFT.replace("{}", name_str).into_bytes();

        // signal to peer b client that peer a has left
        self.peer_b_client_tx.send(PeerMsg::Leave(leave_msg)).await
            .expect("Unable to tx");
    }
}

impl PeerServerWriter {
    // loop to handle writes back to peer client e.g. PeerA
    // PeerB is written to via client tx sender as PeerB is this node
    async fn handle_write(&mut self) {
        let input = self.tcp_write.take().unwrap();
        let mut fw = FramedWrite::new(input, ChatCodec);
        let mut reply;

        loop {
            select! {
            // Read from local read channel, data received from tcp client peer a
                Some(msg_a) = self.local_rx.recv() => {
                    debug!("Peer server A - received in its local msg queue: {:?}", &msg_a);

                    match msg_a {
                        PeerMsg::Hello(_, m) => {
                            reply = Reply::Hello(m);
                            fw.send(reply).await.expect("Unable to write to server")
                        },
                        PeerMsg::Leave(_) => return,
                        _ => unimplemented!(),
                    }
                }
                Some(msg_b) = self.peer_b_server_rx.recv() => {
                    debug!("Peer server B - received in its local client-server msg queue: {:?}", &msg_b);
                    // handle messages sent from this local peer b node's cmd line
                    match msg_b {
                        PeerMsg::Leave(m) => { // case where peer b (as opposed to peer a) wants to leave
                            reply = Reply::Leave(m);
                            fw.send(reply).await.expect("Unable to write to server");
                            return
                        },
                        PeerMsg::Note(m) => {
                            reply = Reply::Note(m);
                            fw.send(reply).await.expect("Unable to write to server")
                        },
                        _ => unimplemented!(),
                    }
                }
                else => {
                    return
                }
            }
        }
    }
}
