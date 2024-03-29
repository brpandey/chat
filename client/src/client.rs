//! Main client that connects to main rendezvous server
//! *Performs client registration with main server
//! *Spawns peer clients if user specifies fork cmd
//! *If Main server terminates, client subsequently terminates leaving any active peer sessions untouched

use tokio::select;
use tokio::io::{self}; 
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait
use tokio::task::JoinHandle;
use tokio::sync::broadcast::{Sender as BSender};

use protocol::{ChatMsg, Request, Response};

use crate::types::{EventMsg, ClientError, ReaderError};
use crate::input_reader::{session_id, InputReader};
use crate::input_shared::InputShared;
use crate::event_bus::EventBus;

use crate::builder::{Builder, ClientBuilder, ConnectType};
use crate::peer_set::PeerNames;
use crate::peer::director::{Peer, PeerA};
use crate::peer::server::{PeerServerListener, PEER_SERVER, PeerServer};

use tracing::{/*info,*/ debug, error};

const GREETINGS: &str = "$ Welcome to chat! \n$ Commands: \\quit, \\users, \\fork chatname, \\switch n, \\sessions\nNote: fork and users command require that you are in the lobby session e.g. \\lobby\n$ Please input chat name: ";
const MAIN_SERVER: &str = "127.0.0.1:43210";

const SHUTDOWN_SERVER: u8 = 1;
const SHUTDOWN_QUIT: u8 = 2;
const SHUTDOWN_IO_DOWN: u8 = 3;

pub struct Client {
    id: u16,
    io_id: u16,
    name: String,
    builder: ClientBuilder,
}

impl Client {

    /*** Associated functions ***/

    pub fn new(io_id: u16, builder: ClientBuilder) -> Self {
        Client {
            id: 0,
            io_id,
            name: String::new(),
            builder,
        }
    }

    // Utilize builder to construct client target structure
    pub async fn build(io_shared: &InputShared) -> io::Result<Client> {
        let mut cb = ClientBuilder::new();
        cb.setup_connection(ConnectType::ServerName(MAIN_SERVER.to_string())).await?;
        cb.setup_channels();
        cb.setup_io(io_shared, None).await;

        let client = cb.build();

        Ok(client)
    }

    pub fn spawn(io_shared: InputShared, names: PeerNames, eb: EventBus) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Ok(mut c) = Client::build(&io_shared).await {
                if c.register().await.map_err(|e| {error!("{}", e); e}).is_ok() {
                    c.run(io_shared, names, eb).await.expect("client terminated with an error");
                }
            }
        })
    }

    /*** Method defintions ***/

    pub async fn register(&mut self) -> Result<(), ClientError> {
        let wrapped = InputReader::blocking_read(GREETINGS)
            .map_err(|e| ClientError::ChatNameInputError(e))?;

        if let Some(name) = wrapped {
            self.builder.fw.as_mut().unwrap().send(Request::JoinName(name))
                .await.expect("Unable to write to server");
        }

        let response = self.builder.fr.as_mut().unwrap().next().await
            .ok_or(ClientError::MissingServerJoinHandshake)?
            .map_err(|_| ClientError::MissingServerJoinHandshake)?;

        if let ChatMsg::Server(Response::JoinNameAck{id, name}) = response {
            println!(">>> Registered as name: {}, switch id is {}",
                     std::str::from_utf8(&name).unwrap(), session_id(self.io_id));

            self.name = String::from_utf8(name).unwrap_or_default();
            self.id = id;
        }

        Ok(())
    }

    pub async fn run(&mut self, io_shared: InputShared,
                     names: PeerNames, eb: EventBus) -> io::Result<()> {
        let cmd_line_handle = self.spawn_cmd_line_read(io_shared, names);
        let server_read_handle = self.spawn_read(eb.clone());
        let server_write_handle = self.spawn_write();

        // stagger the local peer server port value
        let addr = format!("{}:{}", PEER_SERVER, PeerServer::peer_port(self.id));

        let (_, mut shutdown_rx) = self.builder.shutdown_handles();
        let (_, shutdown_rx1) = self.builder.shutdown_handles();

        // start up peer server for clients that connect to this node for peer to peer chat.
        let local_server_handle =
            PeerServerListener::spawn_accept(addr, self.name.clone(), shutdown_rx1, eb.clone());

        // If client needs to shut down, close the lobby
        // indicate that only existing peer client conversations are now only running if any
        // should the user type \\sessions command
        if let Ok(value) = shutdown_rx.recv().await {
            debug!("Received shutdown quit, closing lobby {:?}", value);
            if eb.notify(EventMsg::CloseLobby).is_err() {
                error!("unable to send close lobby");
            }

            // if server has closed (e.g. SHUTDOWN_SERVER)
            // don't automatically kill peers, if active
            // allow peer to peer conversations w/o main server being active
            // however if quit than kill all
            if let SHUTDOWN_QUIT | SHUTDOWN_IO_DOWN = value {
                debug!("Received shutdown quit, aborting peer set");
                eb.abort_clients();
            }
        }

        let futures = vec![
            server_read_handle,
            cmd_line_handle,
            server_write_handle,
            local_server_handle,
        ];

        futures::future::join_all(futures).await;

        debug!("Client terminating");

        Ok(())
    }

    pub fn spawn_read(&mut self, eb: EventBus) -> JoinHandle<()> {
        // Spawn client tcp read tokio task, to read back main server msgs
        let client_name = self.name.clone();
        let mut fr = self.builder.take_read();
        let (shutdown_tx, mut shutdown_rx) = self.builder.shutdown_handles();

        tokio::spawn(async move {
            loop {
                select! {
                    // Read lines from server
                    server_input = async {
                        let server_msg = fr.next().await?;
                        debug!("received main server msg, value is {:?}", server_msg);

                        match server_msg {
                            Ok(ChatMsg::Server(Response::UserMessage{id, msg})) => {
                                println!("> {} {}", id, std::str::from_utf8(&msg).unwrap_or_default());
                            },
                            Ok(ChatMsg::Server(Response::Notification(line))) => {
                                println!(">>> {}", std::str::from_utf8(&line).unwrap_or_default());
                            },
                            Ok(ChatMsg::Server(Response::ForkPeerAckA{pid, pname, addr})) => {
                                let peer_name = String::from_utf8(pname).unwrap_or_default();

                                // Spawn tokio task to send client requests to peer server address
                                let addr_str = PeerServer::stagger_address_port(addr, pid);
                                let a = PeerA(addr_str, client_name.clone(), peer_name);
                                eb.notify(EventMsg::Spawn(Peer::PA(a))).expect("Unable to send event msg");
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
                            debug!("Server Remote has closed");
                            shutdown_tx.send(SHUTDOWN_SERVER).expect("Unable to send shutdown");
                            return
                        }
                    }
                    // exit task if shutdown received
                    _ = shutdown_rx.recv() => {
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
                        return;  // exit task if shutdown received
                    }
                }
            }
        })
    }

    pub fn spawn_cmd_line_read(&mut self, io_shared: InputShared, names: PeerNames) -> JoinHandle<()> {
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
                        let mut req = InputReader::read(io_id, &mut input_rx, &io_shared).await?
                            .and_then(|l| Client::parse_input(&name, l, &shutdown_tx));

                        if let Some(Request::ForkPeer{ref pname}) = req {
                            if names.contains(std::str::from_utf8(&pname).unwrap()).await {
                                println!(">>> Unable to fork a duplicate session with same peer!");
                                req = None
                            }
                        }

                        if req.is_some() {
                            local_tx.send(req.unwrap()).await.expect("Unable to tx");
                        }

                        Ok::<_, ReaderError>(())
                    } => {
                        if input.is_err() { // if input handler has received a terminate
                            shutdown_tx.send(SHUTDOWN_IO_DOWN).expect("Unable to send shutdown");
                            return
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        break; // exit task if shutdown received
                    }
                }
            }
        })
    }

    // Parses client specific commands and user text retrieved from input handler
    pub fn parse_input(name: &str, line: String, shutdown_tx: &BSender<u8>) -> Option<Request> {
        match line.as_str() {
            "\\quit" => {
                debug!("Session terminated by user...");
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
                        println!(">>> Unable to fork a session with self");
                        return None
                    } else {
                        return Some(Request::ForkPeer{pname})

                    }
                } else {
                    return None
                }
            },
            l => {
                // if no commands, split up user input
                let msg = InputReader::interleave_newlines(l, None);
                return Some(Request::Message(msg))
            },
        }
    }
}

