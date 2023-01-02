use std::io as stdio;
use std::io::{stdout, Write};
use std::sync::Arc;

use tokio::select;
use tokio::net::{tcp, TcpStream};
use tokio::sync::{Mutex, broadcast};
use tokio::io::{self, Error, ErrorKind}; //, AsyncWriteExt}; //, AsyncReadExt};
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
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
use crate::peer_server::PeerServerListener;

const GREETINGS: &str = "$ Welcome to chat! \n$ Commands: \\quit, \\users, \\fork chatname\n$ Please input chat name: ";
const SERVER: &str = "127.0.0.1:43210";
const PEER_SERVER: &str = "127.0.0.1";
const PEER_SERVER_PORT0: u16 = 43310;
const PEER_SERVER_PORT1: u16 = 43311;
const PEER_SERVER_PORT2: u16 = 43312;
const PEER_SERVER_PORT3: u16 = 43313;
const SHUTDOWN: u8 = 1;
const LINES_MAX_LEN: usize = 256;
const USER_LINES: usize = 64;

pub struct Client {
    id: u16,
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
               fw: Option<FramedWrite<tcp::OwnedWriteHalf, ChatCodec>>) -> Self {

        let (shutdown_tx, shutdown_rx) = broadcast::channel(16);
        let (local_tx, local_rx) = mpsc::channel::<Request>(64);

        Client {
            id: 0,
            name: String::new(),
            peer_clients: Some(Arc::new(Mutex::new(JoinSet::new()))),
            shutdown_tx,
            shutdown_rx,
            fr,
            fw,
            local_tx: Some(local_tx),
            local_rx: Some(local_rx),
        }
    }

    pub async fn setup() -> io::Result<Client> {
        info!("Client starting, connecting to server {:?}", &SERVER);

        let client = TcpStream::connect(SERVER).await
            .map_err(|e| { error!("Unable to connect to server"); e })?;


        //    let server_alive = Arc::new(AtomicBool::new(true));
        //    let server_alive_ref1 = Arc::clone(&server_alive);
        //    let server_alive_ref2 = Arc::clone(&server_alive);

        // split tcpstream so we can hand off to r & w tasks
        let (client_read, client_write) = client.into_split();

        let fr = FramedRead::new(client_read, ChatCodec);
        let fw = FramedWrite::new(client_write, ChatCodec);

        Ok(Client::new(Some(fr), Some(fw)))
    }

    pub async fn register(&mut self) -> io::Result<()> {
        if let Ok(Some(msg)) = read_sync_user_input(GREETINGS) {
            self.fw.as_mut().unwrap().send(msg).await.expect("Unable to write to server");
        } else {
            debug!("Unable to retrieve user chat name");
            return Err(Error::new(ErrorKind::Other, "Unable to retrieve user chat name"));
        }

        if let Some(Ok(ChatMsg::Server(Response::JoinNameAck{id, name}))) = self.fr.as_mut().unwrap().next().await {
            println!(">>> Registered as name: {}", std::str::from_utf8(&name).unwrap());
            self.name = String::from_utf8(name).unwrap_or_default();
            self.id = id;
        } else {
            return Err(Error::new(ErrorKind::Other, "Didn't receive JoinNameAck"));
        }

        Ok(())
    }

    pub fn spawn() -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Ok(mut c) = Client::setup().await {
                if c.register().await.is_ok() {
                    c.run().await.expect("client terminated with an error");
                }
            }
        })
    }

    pub async fn run(&mut self) -> io::Result<()>{
        self.spawn_read();
        self.spawn_cmd_line_read();
        self.spawn_write();

        // stagger the local peer server port value
        let port = peer_port(self.id);
        let addr = format!("{}:{}", PEER_SERVER, port);

        // start up peer server for clients that connect to this node for peer to peer chat.
        PeerServerListener::spawn_accept(addr, self.name.clone());

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


    pub fn spawn_read(&mut self) {
        // Spawn client tcp read tokio task, to read back main server msgs
        let client_name = self.name.clone();
        let peer_set = Arc::clone(self.peer_clients.as_mut().unwrap());
        let shutdown_tx = self.shutdown_tx.clone();
//        let shutdown_rx = self.shutdown_tx.subscribe();
        let mut fr = self.fr.take().unwrap();

        let _server_read_handle = tokio::spawn(async move {
            loop {
                debug!("task A: listening to server replies...");

                select! {
                    // Read lines from server
                    Some(value) = fr.next() => {
                        debug!("received server value is {:?}", value);

                        match value {
                            Ok(ChatMsg::Server(Response::UserMessage{id, msg})) => {
                                println!("> {} {}", id, std::str::from_utf8(&msg).unwrap());
                            },
                            Ok(ChatMsg::Server(Response::Notification(line))) => {
                                println!(">>> {}", std::str::from_utf8(&line).unwrap());
                            },
                            Ok(ChatMsg::Server(Response::ForkPeerAckA{id, name, mut addr})) => {
                                println!(">>> About to fork private session with {} {}", id, std::str::from_utf8(&name).unwrap());

                                // spawn tokio task to send client requests to peer server address
                                // responding to server requests will take a lower priority
                                // unless the user explicitly switches over to communicating with the server from
                                // peer to peer mode

                                // drop current port of addr
                                // (since this is used already)
                                // add a staggered port to addr
                                addr.set_port(peer_port(id));
                                let addr_string = format!("{}", &addr);
                                peer_set.lock().await.spawn(PeerClient::nospawn_a(addr_string, client_name.clone()));

                                // should probably set some cond var that blocks other threads from reading from stdin
                                // until user explicitly switches to client -> server mode
                            },
                            Ok(ChatMsg::Server(Response::PeerUnavailable(name))) => {
                                println!(">>> Unable to fork into private session as peer {} unavailable",
                                         std::str::from_utf8(&name).unwrap());
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

    pub fn spawn_cmd_line_read(&mut self) {
        let local_tx = self.local_tx.take().unwrap();
        let shutdown_tx = self.shutdown_tx.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        // Use current thread to loop and grab data from command line
        let _cmd_line_handle = tokio::spawn(async move {
            loop {
                debug!("task C: cmd line input looping");
                select! {
                    Ok(Some(msg)) = read_async_user_input() => {
                        local_tx.send(msg).await.expect("xxx Unable to tx");
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
}


// blocking function to gather user input from std::io::stdin
fn read_sync_user_input(prompt: &str) -> io::Result<Option<Request>> {
    let mut buf = String::new();

    print!("{} ", prompt);
    stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
    stdio::stdin().read_line(&mut buf).expect("unable to read command line input");

    let name = buf.trim_end().as_bytes().to_owned();

    Ok(Some(Request::JoinName(name)))
}

async fn read_async_user_input() -> io::Result<Option<Request>> {
    let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new_with_max_length(LINES_MAX_LEN));

    if let Some(Ok(line)) = fr.next().await {
        // handle user inputted commands
        match line.as_str() {
            "\\quit" => {
                info!("Session terminated by user...");
                return Ok(Some(Request::Quit));
            },
            "\\users" => {
                return Ok(Some(Request::Users))
            },
            value if value.starts_with("\\fork") => {
                if let Some(name) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let name: Vec<u8> = name.as_bytes().to_owned();
                    info!("Attempting to fork a session with {}", std::str::from_utf8(&name).unwrap());
                    // todo -- local validation if name is present
                    return Ok(Some(Request::ForkPeer{name}));
                }
            },
            _ => (),
        }

        // if no commands, split up user input
        let mut total = vec![];
        let mut citer = line.as_bytes().chunks(USER_LINES);

        while let Some(c) = citer.next() {
            let mut line = c.to_vec();
            line.push(b'\n');
            total.push(line);
        }

        let msg: Vec<u8> = total.into_iter().flatten().collect();
        return Ok(Some(Request::Message(msg)))
    }

    Ok(None)
}

// given num spread the port to use amongst n values, where n is 4
pub fn peer_port(num: u16) -> u16 {
    match num % 4 {
        0 => PEER_SERVER_PORT0,
        1 => PEER_SERVER_PORT1,
        2 => PEER_SERVER_PORT2,
        3 => PEER_SERVER_PORT3,
        _ => PEER_SERVER_PORT0,
    }
}
