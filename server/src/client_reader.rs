use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::mpsc::Sender;
use tokio_util::codec::{FramedRead, LinesCodec};
//use tokio::io::{AsyncReadExt};
use tokio_stream::StreamExt;

use tracing::{info, debug};

use crate::server::MsgType;
use crate::server::Registry;
use crate::names::NamesShared;

const LINES_MAX_LEN: usize = 256;
const USER_LEFT: &str = "User {} has left\n";




// Handles server communication from client
pub struct ClientReader {
    client_id: usize,
    tcp_read: Option<OwnedReadHalf>,
    task_tx: Sender<MsgType>,
    clients: Registry,
    chat_names: NamesShared,
    msg_prefix: Vec<u8>,
}

impl ClientReader {

    pub fn new(client_id: usize, tcp_read: OwnedReadHalf, task_tx: Sender<MsgType>, clients: Registry,
               chat_names: NamesShared, msg_prefix: Vec<u8>) -> Self {
        Self {
            client_id,
            tcp_read: Some(tcp_read),
            task_tx,
            clients,
            chat_names,
            msg_prefix,
        }
    }

/*
    // register client properly
    pub fn register(&mut self) {

    }
*/

    // Spawn tokio task to handle server socket reads from clients
    pub fn spawn(mut client_reader: ClientReader) {
        let _h = tokio::spawn(async move {
            client_reader.handle_read().await;
        });
    }


    async fn client_disconnected(&mut self) {
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

    async fn handle_read(&mut self) {
        let input = self.tcp_read.take().unwrap();
        let mut fr = FramedRead::new(input, LinesCodec::new_with_max_length(LINES_MAX_LEN));

        loop {
            // Read lines from server
            if let Some(value) = fr.next().await {
                debug!("server received: {:?}", value);

                match value {
                    Ok(line) if line == "users" => self.task_tx.send(MsgType::Users(self.client_id)).await.expect("Unable to tx"),
                    Ok(line) if line.len() > 0 => {
                        let mut msg: Vec<u8> = Vec::with_capacity(self.msg_prefix.len() + line.len() + 1);
                        let mut p = self.msg_prefix.clone();
                        let mut line2 = line.into_bytes();
                        msg.append(&mut p);
                        msg.append(&mut line2);
                        msg.push(b'\n');

                        debug!("Server received client msg {:?}", &msg);

                        // delegate the broadcast of msg echoing to another block
                        self.task_tx.send(MsgType::Message(self.client_id, msg)).await
                            .expect("Unable to tx");
                    },
                    Ok(_) => continue,
                    Err(x) => {
                        debug!("Server Connection closing error: {:?}", x);
                        break;
                    },
                }
            } else {
                self.client_disconnected().await;
                break;
            }
        }
    }
}
