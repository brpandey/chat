use tokio::select;
use tokio::io::{self, Error, ErrorKind}; //, AsyncWriteExt}; //, AsyncReadExt};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait
use tokio::task::JoinHandle;
use tokio::sync::broadcast::{Sender as BSender};

//use tracing_subscriber::fmt;
use tracing::{info, debug, error};

use protocol::{ChatMsg, Request, Response};

use crate::builder::ClientBuilder as Builder;
use crate::peer_set::{PeerSet, PeerSetShared};
use crate::peer_server::{PeerServerListener, PEER_SERVER, PeerServer};
use crate::types::InputMsg;
use crate::input_reader::{session_id, InputReader};
use crate::input_shared::InputShared;

const GREETINGS: &str = "$ Welcome to chat! \n$ Commands: \\quit, \\users, \\fork chatname, \\switch n, \\sessions\n$ Please input chat name: ";
const MAIN_SERVER: &str = "127.0.0.1:43210";

const SHUTDOWN_SERVER: u8 = 1;
const SHUTDOWN_QUIT: u8 = 2;
const SHUTDOWN_IO_DOWN: u8 = 3;

pub struct Client {
    id: u16,
    io_id: u16,
    name: String,
    builder: Builder,
}

impl Client {
    pub fn new(io_id: u16, builder: Builder) -> Self {
        Client {
            id: 0,
            io_id,
            name: String::new(),
            builder,
        }
    }

    // Utilize builder to construct client target structure
    pub async fn build(io_shared: &InputShared) -> io::Result<Client> {
        let client = Builder::new()
            .connect(&MAIN_SERVER).await?
            .channels()
            .io_register(io_shared).await
            .build();

        Ok(client)
    }

    pub async fn register(&mut self) -> io::Result<()> {
        if let Ok(Some(name)) = InputReader::blocking_read(GREETINGS) {
            self.builder.fw.as_mut().unwrap().send(Request::JoinName(name))
                .await.expect("Unable to write to server");
        } else {
            debug!("Unable to retrieve user chat name");
            return Err(Error::new(ErrorKind::Other, "Unable to retrieve user chat name"));
        }

        if let Some(Ok(ChatMsg::Server(Response::JoinNameAck{id, name}))) = self.builder.fr.as_mut().unwrap().next().await {
            println!(">>> Registered as name: {}, switch id is {}",
                     std::str::from_utf8(&name).unwrap(), session_id(self.io_id));

            self.name = String::from_utf8(name).unwrap_or_default();
            self.id = id;
        } else {
            return Err(Error::new(ErrorKind::Other, "Didn't receive JoinNameAck"));
        }

        Ok(())
    }

    pub fn spawn(io_shared: InputShared, peer_set: PeerSet) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Ok(mut c) = Client::build(&io_shared).await {
                if c.register().await.is_ok() {
                    c.run(io_shared, peer_set).await.expect("client terminated with an error");
                }
            }
        })
    }

    pub async fn run(&mut self, io_shared: InputShared, mut peer_set: PeerSet) -> io::Result<()> {
        let cmd_line_handle = self.spawn_cmd_line_read(io_shared.clone(), peer_set.get_shared());
        let server_read_handle = self.spawn_read(io_shared.clone(), peer_set.clone());
        let server_write_handle = self.spawn_write();

        // stagger the local peer server port value
        let addr = format!("{}:{}", PEER_SERVER, PeerServer::peer_port(self.id));

        let (_, shutdown_rx1) = self.builder.shutdown_handles();
        let (_, mut shutdown_rx2) = self.builder.shutdown_handles();

        // start up peer server for clients that connect to this node for peer to peer chat.
        let local_server_handle =
            PeerServerListener::spawn_accept(addr, self.name.clone(), io_shared.clone(), peer_set.clone(), shutdown_rx1);

        // If client needs to shut down, close the lobby
        // indicate that only existing peer client conversations are now only running if any
        // should the user type \\sessions command
        if let Ok(value) = shutdown_rx2.recv().await {
            info!("Received shutdown quit, closing lobby {:?}", value);
            if io_shared.notify(InputMsg::CloseLobby).await.is_err() {
                error!("unable to send close lobby");
            }

            // if server has closed (e.g. SHUTDOWN_SERVER)
            // don't automatically kill peers, if active
            // allow peer to peer conversations w/o main server being active
            // however if quit than kill all
            if value == SHUTDOWN_QUIT || value == SHUTDOWN_IO_DOWN {
                info!("Received shutdown quit, aborting peer set");
                peer_set.abort_all();
            }
        }

        let futures = vec![
            server_read_handle,
            cmd_line_handle,
            server_write_handle,
            local_server_handle,
        ];

        futures::future::join_all(futures).await;

        info!("Client terminating: either server was terminated or user terminated by ctrl c'ing or \\quit");

        Ok(())
    }

    pub fn spawn_read(&mut self, io_shared: InputShared, mut peer_set: PeerSet) -> JoinHandle<()> {
        // Spawn client tcp read tokio task, to read back main server msgs
        let client_name = self.name.clone();

        let mut fr = self.builder.take_read();
        let (shutdown_tx, mut shutdown_rx) = self.builder.shutdown_handles();

        tokio::spawn(async move {
            loop {
                debug!("task A: listening to server replies...");

                select! {
                    // Read lines from server
                    server_input = async {
                        let server_msg = fr.next().await?;
                        debug!("received server value is {:?}", server_msg);

                        match server_msg {
                            Ok(ChatMsg::Server(Response::UserMessage{id, msg})) => {
                                println!("> {} {}", id, std::str::from_utf8(&msg).unwrap_or_default());
                            },
                            Ok(ChatMsg::Server(Response::Notification(line))) => {
                                println!(">>> {}", std::str::from_utf8(&line).unwrap_or_default());
                            },
                            Ok(ChatMsg::Server(Response::ForkPeerAckA{pid, pname, addr})) => {
                                let peer_name = String::from_utf8(pname).unwrap_or_default();
                                println!(">>> Forked private session with {} {}", pid, peer_name);
                                println!(">>> To switch back to main lobby, type: \\sw 0");

                                // Spawn tokio task to send client requests to peer server address
                                let addr_str = PeerServer::stagger_address_port(addr, pid);
                                peer_set.spawn_peer_a(addr_str, client_name.clone(), peer_name, io_shared.clone()).await;
                            },
                            Ok(ChatMsg::Server(Response::PeerUnavailable(name))) => {
                                println!(">>> Unable to fork into private session as peer {} unavailable",
                                         std::str::from_utf8(&name).unwrap_or_default());
                            },
                            Ok(_) => unimplemented!(),
                            Err(x) => {
                                debug!("Client Connection closing error: {:?}", x);
                            }
                        }
                        Some(())
                    } => {
                        if server_input.is_none() { // if input handler has received a terminate
                            info!("!!Server Remote has closed, sending shutdown msg A");
                            shutdown_tx.send(SHUTDOWN_SERVER).expect("Unable to send shutdown");
                            return
                        }
                    }
                    // exit task if shutdown received
                    _ = shutdown_rx.recv() => {
                        info!("server_read_handle received shutdown, returning!");
                        return;
                    }
                };
            }
        })
    }

    pub fn spawn_write(&mut self) -> JoinHandle<()> {
        let (mut fw, mut local_rx) = self.builder.take_write_fields();
        let (_, mut shutdown_rx) = self.builder.shutdown_handles();

        // Spawn client tcp write tokio task, to send data to server
        tokio::spawn(async move {
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
                }
            }
        })
    }

    pub fn spawn_cmd_line_read(&mut self, io_shared: InputShared, ps_shared: PeerSetShared) -> JoinHandle<()> {
        // get self field
        let name = self.name.clone();

        // grab io related data
        let io_id = self.io_id;
        let mut input_rx = io_shared.get_receiver();

        // get builder fields
        let local_tx = self.builder.take_tx();
        let (shutdown_tx, mut shutdown_rx) = self.builder.shutdown_handles();

        // Use current thread to loop and grab data from command line
        tokio::spawn(async move {
            loop {
                select! {
                    input = async {
                        let req = InputReader::read(io_id, &mut input_rx, &io_shared).await?
                            .and_then(|l| Client::parse_input(&name, l, &shutdown_tx));

                        if req.is_some() {
                            if let Some(Request::ForkPeer{ref pname}) = req {
                                if ps_shared.contains(std::str::from_utf8(&pname).unwrap()).await {
                                    info!("Unable to fork a duplicate session with same peer!");
                                } else {
                                    local_tx.send(req.unwrap()).await.expect("xxx Unable to tx");
                                }
                            } else {
                                local_tx.send(req.unwrap()).await.expect("xxx Unable to tx");
                            }
                        }

                        Ok::<_, io::Error>(())
                    } => {
                        if input.is_err() { // if input handler has received a terminate
                            info!("sending shutdown msg C");
                            shutdown_tx.send(SHUTDOWN_IO_DOWN).expect("Unable to send shutdown");
                            return
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("cmd_line_handle received shutdown, returning!");
                        break; // exit task if shutdown received
                    }
                }
            }
        })
    }

    pub fn parse_input(name: &str, line: String, shutdown_tx: &BSender<u8>) -> Option<Request> {
        match line.as_str() {
            "\\quit" => {
                info!("Session terminated by user...");
                shutdown_tx.send(SHUTDOWN_QUIT).expect("Unable to send shutdown");
                return Some(Request::Quit)
            },
            "\\users" => {
                return Some(Request::Users)
            },
            value if value.starts_with("\\fork") => {
                if let Some(pname_str) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let pname: Vec<u8> = pname_str.as_bytes().to_owned();
                    if name.as_bytes() == pname {
                        info!("Only able to fork a session with other peers, not current peer!");
                        return None
                    } else {
                        info!("Attempting to fork a session with {}", std::str::from_utf8(&pname).unwrap_or_default());
                        return Some(Request::ForkPeer{pname})

                    }
                } else {
                    return None
                }
            },
            l => {
                // if no commands, split up user input
                let msg = InputReader::interleave_newlines(l, vec![]);
                return Some(Request::Message(msg))
            },
        }
    }
}

