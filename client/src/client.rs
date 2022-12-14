use tokio::select;
use tokio::io::{self, Error, ErrorKind}; //, AsyncWriteExt}; //, AsyncReadExt};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait
use futures::SinkExt; // provides combinator methods like send/send_all on top of FramedWrite buf write and Sink trait
use tokio::task::JoinHandle;

//use tracing_subscriber::fmt;
use tracing::{info, debug, /*error*/};

use protocol::{ChatMsg, Request, Response};

use crate::builder::ClientBuilder as Builder;
use crate::peer_set::PeerSet;
use crate::peer_server::{PeerServerListener, PEER_SERVER, PeerServer};
use crate::input_handler::{IO_ID_OFFSET, InputHandler, InputShared};

const GREETINGS: &str = "$ Welcome to chat! \n$ Commands: \\quit, \\users, \\fork chatname, \\switch n\n$ Please input chat name: ";
const MAIN_SERVER: &str = "127.0.0.1:43210";
const SHUTDOWN: u8 = 1;

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
        if let Ok(Some(name)) = InputHandler::read_sync_user_input(GREETINGS) {
            self.builder.fw.as_mut().unwrap().send(Request::JoinName(name))
                .await.expect("Unable to write to server");
        } else {
            debug!("Unable to retrieve user chat name");
            return Err(Error::new(ErrorKind::Other, "Unable to retrieve user chat name"));
        }

        if let Some(Ok(ChatMsg::Server(Response::JoinNameAck{id, name}))) = self.builder.fr.as_mut().unwrap().next().await {
            println!(">>> Registered as name: {}, switch id is {}", std::str::from_utf8(&name).unwrap(), self.io_id - IO_ID_OFFSET);
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
        self.spawn_cmd_line_read(io_shared.clone());
        self.spawn_read(io_shared.clone(), peer_set.clone());
        self.spawn_write();

        // stagger the local peer server port value
        let addr = format!("{}:{}", PEER_SERVER, PeerServer::peer_port(self.id));

        // start up peer server for clients that connect to this node for peer to peer chat.
        PeerServerListener::spawn_accept(addr, self.name.clone(), io_shared, peer_set);

        let (_, mut shutdown_rx) = self.builder.shutdown_handles();

        select! {
            _ = shutdown_rx.recv() => { // exit task if shutdown received
                info!("received final shutdown");
//                break;
            }
        };

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

        info!("Thank you and good bye!");

        Ok(())
    }

    pub fn spawn_read(&mut self, io_shared: InputShared, mut peer_set: PeerSet) {
        // Spawn client tcp read tokio task, to read back main server msgs
        let client_name = self.name.clone();

        let mut fr = self.builder.take_read();
        let (shutdown_tx, mut shutdown_rx) = self.builder.shutdown_handles();

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
                            Ok(ChatMsg::Server(Response::ForkPeerAckA{pid, name, addr})) => {
                                let peer_name = String::from_utf8(name).unwrap_or_default();
                                println!(">>> Forked private session with {} {}", pid, peer_name);
                                println!(">>> To switch back to main lobby, type: \\s 0");

                                // Spawn tokio task to send client requests to peer server address
                                let addr_str = PeerServer::stagger_address_port(addr, pid);
                                peer_set.spawn_a(addr_str, client_name.clone(), peer_name, io_shared.clone()).await;
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
                    _ = shutdown_rx.recv() => {
                        info!("server_read_handle received shutdown, returning!");
                        return;
                    }
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
        let (mut fw, mut local_rx) = self.builder.take_write_fields();
        let (_, mut shutdown_rx) = self.builder.shutdown_handles();

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
                }
            }
        });
    }

    pub fn spawn_cmd_line_read(&mut self, io_shared: InputShared) {
        // grab io related data
        let io_id = self.io_id;
        let mut input_rx = io_shared.get_receiver();

        // get builder fields
        let local_tx = self.builder.take_tx();
        let (shutdown_tx, mut shutdown_rx) = self.builder.shutdown_handles();

        // Use current thread to loop and grab data from command line
        let _cmd_line_handle = tokio::spawn(async move {
            loop {
                select! {
                    input = async {
                        let req = InputHandler::read_async_user_input(io_id, &mut input_rx, &io_shared).await?
                            .and_then(Client::parse_input);

                        if req.is_some() {
                            local_tx.send(req.unwrap()).await.expect("xxx Unable to tx");
                        }

                        Ok::<_, io::Error>(())
                    } => {
                        if input.is_err() { // if input handler has received a terminate
                            info!("sending shutdown msg C");
                            shutdown_tx.send(SHUTDOWN).expect("Unable to send shutdown");
                            return
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("cmd_line_handle received shutdown, returning!");
                        break; // exit task if shutdown received
                    }
                }
            }
        });
    }

    pub fn parse_input(line: String) -> Option<Request> {
        match line.as_str() {
            "\\quit" => {
                info!("Session terminated by user...");
                return Some(Request::Quit)
            },
            "\\users" => {
                return Some(Request::Users)
            },
            value if value.starts_with("\\fork") => {
                if let Some(name) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let name: Vec<u8> = name.as_bytes().to_owned();
                    info!("Attempting to fork a session with {}", std::str::from_utf8(&name).unwrap_or_default());
                    return Some(Request::ForkPeer{name})
                } else {
                    return None
                }
            },
            l => {
                // if no commands, split up user input
                let msg = InputHandler::interleave_newlines(l, vec![]);
                return Some(Request::Message(msg))
            },
        }
    }
}

