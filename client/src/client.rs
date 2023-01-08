use std::sync::{Arc, /*Mutex*/};

use tokio::select;
use tokio::net::{tcp, TcpStream};
use tokio::sync::Mutex;
use tokio::sync::{broadcast};

use tokio::io::{self, Error, ErrorKind}; //, AsyncWriteExt}; //, AsyncReadExt};
use tokio_util::codec::{FramedRead, FramedWrite, /*LinesCodec*/};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait
use tokio::task::{JoinSet, JoinHandle};
use tokio::sync::mpsc::{self, Sender, Receiver};
use tokio::sync::broadcast::Sender as BSender;
use tokio::sync::broadcast::Receiver as BReceiver;

//use tracing_subscriber::fmt;
use tracing::{info, debug, error};

use protocol::{ChatMsg, ChatCodec, Request, Response};

use crate::peer_client::PeerClient;
use crate::peer_server::{PeerServerListener, PEER_SERVER, peer_port};
use crate::input_handler::{IO_ID_OFFSET, InputHandler, InputShared};

const GREETINGS: &str = "$ Welcome to chat! \n$ Commands: \\quit, \\users, \\fork chatname, \\switch n\n$ Please input chat name: ";
const MAIN_SERVER: &str = "127.0.0.1:43210";
const SHUTDOWN: u8 = 1;

pub struct Client {
    id: u16,
    io_id: u16,
    name: String,
    peer_clients: Option<Arc<Mutex<JoinSet<u8>>>>,
    shutdown_tx: BSender<u8>,
    shutdown_rx: BReceiver<u8>,
    fr: Option<FramedRead<tcp::OwnedReadHalf, ChatCodec>>,
    fw: Option<FramedWrite<tcp::OwnedWriteHalf, ChatCodec>>,
    local_rx: Option<Receiver<Request>>,
    local_tx: Option<Sender<Request>>,
}

impl Client {
    pub fn new(fr: Option<FramedRead<tcp::OwnedReadHalf, ChatCodec>>,
               fw: Option<FramedWrite<tcp::OwnedWriteHalf, ChatCodec>>, io_id: u16) -> Self {

        let (shutdown_tx, shutdown_rx) = broadcast::channel(16);
        let (local_tx, local_rx) = mpsc::channel::<Request>(64);
        let peer_clients = Some(Arc::new(Mutex::new(JoinSet::new())));

        Client {
            id: 0,
            io_id,
            name: String::new(),
            peer_clients,
            shutdown_tx,
            shutdown_rx,
            fr,
            fw,
            local_tx: Some(local_tx),
            local_rx: Some(local_rx),
        }
    }

    pub async fn setup(io_shared: &InputShared) -> io::Result<Client> {
        info!("Client starting, connecting to server {:?}", &MAIN_SERVER);

        let client = TcpStream::connect(MAIN_SERVER).await
            .map_err(|e| { error!("Unable to connect to server"); e })?;

        // split tcpstream so we can hand off to r & w tasks
        let (client_read, client_write) = client.into_split();

        let fr = FramedRead::new(client_read, ChatCodec);
        let fw = FramedWrite::new(client_write, ChatCodec);

        let io_id = io_shared.get_next_id().await;

        Ok(Client::new(Some(fr), Some(fw), io_id))
    }

    pub async fn register(&mut self) -> io::Result<()> {
        if let Ok(Some(name)) = InputHandler::read_sync_user_input(GREETINGS) {
            self.fw.as_mut().unwrap().send(Request::JoinName(name))
                .await.expect("Unable to write to server");
        } else {
            debug!("Unable to retrieve user chat name");
            return Err(Error::new(ErrorKind::Other, "Unable to retrieve user chat name"));
        }

        if let Some(Ok(ChatMsg::Server(Response::JoinNameAck{id, name}))) = self.fr.as_mut().unwrap().next().await {
            println!(">>> Registered as name: {}, switch id is {}", std::str::from_utf8(&name).unwrap(), self.io_id - IO_ID_OFFSET);
            self.name = String::from_utf8(name).unwrap_or_default();
            self.id = id;
        } else {
            return Err(Error::new(ErrorKind::Other, "Didn't receive JoinNameAck"));
        }

        Ok(())
    }

    pub fn spawn(io_shared: InputShared) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Ok(mut c) = Client::setup(&io_shared).await {
                if c.register().await.is_ok() {
                    c.run(io_shared).await.expect("client terminated with an error");
                }
            }
        })
    }

    pub async fn run(&mut self, io_shared: InputShared) -> io::Result<()> {
        self.spawn_cmd_line_read(io_shared.clone());
        self.spawn_read(io_shared.clone());
        self.spawn_write();

        // stagger the local peer server port value
        let addr = format!("{}:{}", PEER_SERVER, peer_port(self.id));

        // start up peer server for clients that connect to this node for peer to peer chat.
        PeerServerListener::spawn_accept(addr, self.name.clone(), io_shared);

        loop {
            select! {
                _ = self.shutdown_rx.recv() => { // exit task if shutdown received
                    info!("received final shutdown");
                    break;
                }
            };
        }

/*
        select! {
            _ = server_read_handle => { // fires as soon as future completes
                //      tcp_write_handle.abort();
                println!("X");
            }
            _ = cmd_line_handle => {
                println!("Y");
            }
        }
*/

        // consume arc and lock
        let peer_clients = self.peer_clients.take().unwrap();
       // thread 'tokio-runtime-worker' panicked at 'called `Result::unwrap()` on an `Err` value: Mutex { data: JoinSet { len: 1 } }', src/client.rs:143:57
        let mut clients = Arc::try_unwrap(peer_clients).unwrap().into_inner();

        info!("clients are {:?}", clients);

        while let Some(res) = clients.join_next().await {
            info!("peer client completed {:?}", res);
        }

        info!("waka waka!");

        Ok(())
    }


    pub fn spawn_read(&mut self, io_shared: InputShared) {
        // Spawn client tcp read tokio task, to read back main server msgs
        let client_name = self.name.clone();
        let peer_set = Arc::clone(self.peer_clients.as_mut().unwrap());
        let shutdown_tx = self.shutdown_tx.clone();
//        let shutdown_rx = self.shutdown_tx.subscribe();
        let mut fr = self.fr.take().unwrap();
//        let client_id = self.id;

        let _server_read_handle = tokio::spawn(async move {
            loop {
                debug!("task A: listening to server replies...");

                select! {
                    // Read lines from server
                    Some(value) = fr.next() => {
                        debug!("received server value is {:?}", value);

                        match value {
                            Ok(ChatMsg::Server(Response::UserMessage{id, msg})) => {
                                println!("> {} {}", id, std::str::from_utf8(&msg).unwrap_or_default());
                            },
                            Ok(ChatMsg::Server(Response::Notification(line))) => {
                                println!(">>> {}", std::str::from_utf8(&line).unwrap_or_default());
                            },
                            Ok(ChatMsg::Server(Response::ForkPeerAckA{pid, name, mut addr})) => {
                                let peer_name = String::from_utf8(name).unwrap_or_default();
                                println!(">>> Forked private session with {} {}", pid, peer_name);

                                println!(">>> To switch back to main lobby, type: \\s 0");

                                // spawn tokio task to send client requests to peer server address
                                // responding to server requests will take a lower priority
                                // unless the user explicitly switches over to communicating with the server from
                                // peer to peer mode

                                // drop current port of addr
                                // (since this is used already)
                                // add a staggered port to addr
                                addr.set_port(peer_port(pid));
                                let addr_string = format!("{}", &addr);

                                peer_set
                                    .lock().await
                                    .spawn(PeerClient::nospawn_a(addr_string, client_name.clone(), peer_name, io_shared.clone()));

                                // should probably set some cond var that blocks other threads from reading from stdin
                                // until user explicitly switches to client -> server mode
                            },
                            Ok(ChatMsg::Server(Response::PeerUnavailable(name))) => {
                                println!(">>> Unable to fork into private session as peer {} unavailable",
                                         std::str::from_utf8(&name).unwrap_or_default());
                            },
                            Ok(_) => unimplemented!(),
                            Err(x) => {
                                debug!("Client Connection closing error: {:?}", x);
                                break;
                            }
                        }
                    } // exit task if shutdown received

                    /*
                    _ = shutdown_rx.recv() => {
                        info!("server_read_handle received shutdown, returning!");
                        return;
                    }
                     */
                    else => {
                        info!("Server Remote has closed");
                        info!("sending shutdown msg A");
                        shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                        break;
                    }
                };
                //        server_alive_ref1.swap(false, Ordering::Relaxed);
            }
        });
    }

    pub fn spawn_write(&mut self) {
        let mut local_rx = self.local_rx.take().unwrap();
//        let shutdown_tx = self.shutdown_tx.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut fw = self.fw.take().unwrap();

        // Spawn client tcp write tokio task, to send data to server
        let _tcp_write_handle = tokio::spawn(async move {
            loop {
                select! {
                    // Read from channel, data received from command line
                    Some(msg) = local_rx.recv() => {
                        fw.send(msg).await.expect("Unable to write to server");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("tcp_write_handle received shutdown, returning!");
                        return;  // exit task if shutdown received
                    }
                    /*
                    else => {
                        shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                        info!("sending shutdown msg B");
                        break;
                    }
                     */
                }
            }
        });
    }

    pub fn spawn_cmd_line_read(&mut self, io_shared: InputShared) {
        let local_tx = self.local_tx.take().unwrap();
        let shutdown_tx = self.shutdown_tx.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut input_rx = io_shared.get_receiver();
        let io_id = self.io_id;

        // Use current thread to loop and grab data from command line
        let _cmd_line_handle = tokio::spawn(async move {
            loop {
                debug!("task C: cmd line input looping");

                select! {
                    Ok(msg) = InputHandler::read_async_user_input(io_id, &mut input_rx, &io_shared) => {
                        let req = Client::parse_input(msg);
                        if req != Request::Noop {
                            local_tx.send(req).await.expect("xxx Unable to tx");
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("cmd_line_handle received shutdown, returning!");
                        break; // exit task if shutdown received
                    }
                    else => {
                        shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                        info!("sending shutdown msg C");
                        break;
                    }
                }

                /*
                // If main server is dead, stop accepting cmd line input
                if !server_alive_ref2.load(Ordering::Relaxed) {
                debug!("Server not alive, hence exiting cmd line processing");
                break;
                 */

            }
        });
    }

    pub fn parse_input(line: Option<String>) -> Request {
        if line.is_none() { return Request::Noop }

        match line.unwrap().as_str() {
            "\\quit" => {
                info!("Session terminated by user...");
                return Request::Quit
            },
            "\\users" => {
                return Request::Users
            },
            value if value.starts_with("\\fork") => {
                if let Some(name) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let name: Vec<u8> = name.as_bytes().to_owned();
                    info!("Attempting to fork a session with {}", std::str::from_utf8(&name).unwrap_or_default());
                    return Request::ForkPeer{name}
                } else {
                    return Request::Noop
                }
            },
            l => {
                // if no commands, split up user input
                let msg = InputHandler::interleave_newlines(l, vec![]);
                return Request::Message(msg)
            },
        }
    }
}

