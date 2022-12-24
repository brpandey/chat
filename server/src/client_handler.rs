use std::net::SocketAddr;
use std::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use tokio::io::{self, Error, ErrorKind, AsyncReadExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc::Sender;
use tokio_util::codec::{Decoder, FramedRead};
use tokio_stream::StreamExt;

use bytes::BytesMut;
use tracing::{info, debug, error};

use protocol::{ChatMsg, ChatCodec, Request};
use crate::server_types::{MsgType, Registry};
use crate::names::NamesShared;

const USER_JOINED: &str = "User {} joined\n";
const USER_JOINED_ACK: &str = "You have successfully joined as {} \n";
const USER_LEFT: &str = "User {} has left\n";

// Handles server communication from client
// Essentially this models a client actor on the server side
pub struct ClientHandler {
    client_id: u16,
    tcp_read: Option<OwnedReadHalf>,
    task_tx: Sender<MsgType>,
    clients: Registry,
    chat_names: NamesShared,
    msg_prefix: Vec<u8>,
}

impl ClientHandler {

    pub fn new(tcp_read: OwnedReadHalf, task_tx: Sender<MsgType>, clients: Registry,
               chat_names: NamesShared) -> Self {
        Self {
            client_id: u16::MAX, // initialized after register()
            tcp_read: Some(tcp_read),
            task_tx,
            clients,
            chat_names,
            msg_prefix: vec![], // initialized after register()
        }
    }

    // Spawn tokio task to handle server socket reads from clients
    pub fn spawn(mut h: ClientHandler, addr: SocketAddr, tcp_write: OwnedWriteHalf, counter: Arc<AtomicU16>) {
        let _ = tokio::spawn(async move {
            // if registration is successful then only handle client reads
            if h.register(addr, tcp_write, counter).await.is_ok() {
                h.handle_read().await;
            }
        });
    }

    // Register client properly: retrieving chat name,
    // but also generate id, and store these data approrpriately
    async fn register(&mut self, addr: SocketAddr, tcp_write: OwnedWriteHalf, counter: Arc<AtomicU16>) -> io::Result<()> {
        let mut client_msg: MsgType;
        let mut name: String = self.read_name().await?;

        self.client_id = counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client
        info!("client_id is {}", self.client_id);

        let addr_str: String;

        // Acquire write lock guard, insert new chat name, using new name if there was a collision
        {
            let mut lg = self.chat_names.write().await;
            addr_str = format!("{}", &addr);
            let addr_bytes = addr_str.as_bytes().to_vec();
            if !lg.insert(name.clone(), (self.client_id, addr_bytes)) { // if collision happened, grab modified handle name
                name = lg.last_collision().unwrap(); // retrieve updated name, has to be not None

                // send msg back acknowledging user joined with updated chat name
                let join_msg = USER_JOINED_ACK.replace("{}", &name);
                client_msg = MsgType::JoinedAck(self.client_id, join_msg.into_bytes());

                // notify others of new join
                self.task_tx.send_timeout(client_msg, Duration::from_millis(75)).await
                    .expect("Unable to tx");
            }
        }

        // Store client data into clients registry
        {
            let mut sc = self.clients.lock().await;
            sc.entry(self.client_id).or_insert((addr_str, name.clone(), tcp_write));
        }

        let join_msg = USER_JOINED.replace("{}", &name);
        client_msg = MsgType::Joined(self.client_id, join_msg.into_bytes());

        // notify others of new join
        self.task_tx.send_timeout(client_msg, Duration::from_millis(75)).await
            .expect("Unable to tx");

        self.set_msg_prompt(name.as_bytes().to_vec());

        Ok(())
    }

    async fn read_name(&mut self) -> io::Result<String> {
        // Wait for chat name msg
        let mut name_buf: Vec<u8> = vec![0; 64];
        let mut chat = ChatCodec;
        let mut bm = BytesMut::new();

        if let Ok(n) = self.tcp_read.as_mut().unwrap().read(&mut name_buf).await {
            name_buf.truncate(n);
            bm.extend_from_slice(&name_buf);
            if let Some(ChatMsg::Client(Request::JoinName(name))) = chat.decode(&mut bm)? {
                return String::from_utf8(name).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid utf8"))
            }
        }

        error!("Unable to retrieve chat user name");
        return Err(Error::new(ErrorKind::Other, "Unable to retrieve chat user name"));
    }

    fn set_msg_prompt(&mut self, mut bytes: Vec<u8>) {
        // Generate per client msg prefix, e.g. "name: "
        self.msg_prefix = Vec::with_capacity(bytes.len() + 2);
        self.msg_prefix.append(&mut bytes);
        self.msg_prefix.push(b':');
        self.msg_prefix.push(b' ');
    }


    // Loop to handle ongoing client msgs to server
    async fn handle_read(&mut self) {
        let input = self.tcp_read.take().unwrap();
        let mut fr = FramedRead::new(input, ChatCodec);

        loop {
            // Read lines from server
            if let Some(value) = fr.next().await {
                info!("server received: {:?}", value);

                match value {
                    Ok(ChatMsg::Client(Request::Users)) =>
                        self.task_tx.send(MsgType::Users(self.client_id)).await.expect("Unable to tx"),
                    Ok(ChatMsg::Client(Request::Quit)) => {
                        self.process_disconnect().await;
                        break;
                    },
                    Ok(ChatMsg::Client(Request::Message(msg))) => {
                        for mut line in msg.split(|e| *e == b'\n').map(|l| l.to_owned()) {
                            if line.is_empty() { continue }

                            let mut msg: Vec<u8> = Vec::with_capacity(self.msg_prefix.len() + line.len() + 1);
                            let mut p = self.msg_prefix.clone();
                            msg.append(&mut p);
                            msg.append(&mut line);

                            // delegate the broadcast of msg echoing to another block
                            self.task_tx.send(MsgType::Message(self.client_id, msg)).await
                                .expect("Unable to tx");
                        }
                    },
                    Ok(ChatMsg::Client(Request::ForkPeer{name})) => {
                        // retrieve client id and addr from names if still active
                        let name_key = std::str::from_utf8(&name).unwrap_or_default();
                        if let Some((peer_id, sock_str)) = self.chat_names.read().await.get(&name_key) {
                            let msg = MsgType::ForkPeerAck(self.client_id, *peer_id, name, sock_str.to_owned());
                            self.task_tx.send(msg).await.expect("Unable to tx");
                        } else {
                            self.task_tx.send(MsgType::PeerUnavailable(self.client_id, name)).await.expect("Unable to tx");
                        }
                    },
                    Ok(_) => unimplemented!(),
                    Err(x) => {
                        debug!("Server Connection closing error: {:?}", x);
                        break;
                    },
                }
            } else {
                self.process_disconnect().await;
                break;
            }
        }
    }

    // process client disconnection event
    async fn process_disconnect(&mut self) {
        info!("Client connection has closed");
        let mut remove_name = String::new();
        let mut leave_msg = vec![];
        { // keep lock scope - mutex guard - small
            let mut mg = self.clients.lock().await;
            if let Some((addr, name, _w)) = mg.remove(&self.client_id) {
                info!("Remote {:?} has closed connection", &addr);
                info!("User {} has left", &name);
                leave_msg = USER_LEFT.replace("{}", &name)
                    .into_bytes();
                remove_name = name;
            }
        }
        // keep names consistent to reflect client is gone
        self.chat_names.write().await.remove(&remove_name);

        // broadcast that user has left
        self.task_tx.send(MsgType::Exited(leave_msg)).await
            .expect("Unable to tx");
    }
}
