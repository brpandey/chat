use std::thread;
use std::io as stdio;
use std::io::{stdout, Write};
use std::io::BufRead;

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::collections::HashMap;

use tokio::io::{self, Error, ErrorKind};
use tokio::select;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender as MSender;
use tokio::sync::mpsc::Receiver as MReceiver;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::watch::{self, Sender, Receiver};

use tracing::info;

pub type InputNotifier = MSender<InputMsg>;
pub type InputReceiver = Receiver<(u16, u16)>;
type InputSender = Sender<(u16, u16)>;

pub const IO_ID_OFFSET: u16 = 1000;
const IO_ID_START: u16 = 1000;
const USER_LINES: usize = 64;

pub type IdLinePair = RwLock<(u16, String)>; // contains the io id along with the current line
pub type Sessions = RwLock<HashMap<u16, String>>;

#[derive(Debug)]
pub enum InputMsg {
    NewSession(u16, String),
    UpdatedSessionName(u16, String),
    CloseSession(u16),
    CloseLobby,
}

#[derive(Debug)]
pub enum InputCmd {
    Quit,
    Switch(u16),
    Sessions,
    Other(String)
}

pub struct InputHandler {
    shared: InputShared,
    watch_tx: Option<InputSender>,
    msg_rx: Option<MReceiver<InputMsg>>,
}

impl InputHandler {
    pub fn new() -> Self {
        let (msg_tx, msg_rx) = mpsc::channel::<InputMsg>(16);
        let (watch_tx, watch_rx) = watch::channel((0, 0)); // Watch channel has 1 tx and potentially multiple receivers
        let shared = InputShared::new(IO_ID_START, watch_rx, msg_tx);

        Self {
            shared,
            watch_tx: Some(watch_tx),
            msg_rx: Some(msg_rx),
        }
    }

    pub fn get_shared(&self) -> InputShared {
        self.shared.clone()
    }

    pub fn interleave_newlines(line: &str, mut acc: Vec<Vec<u8>>) -> Vec<u8> {
        let mut v: Vec<u8>;
        let mut citer = line.as_bytes().chunks(USER_LINES);

        while let Some(chunk) = citer.next() {
            v = chunk.to_vec();
            v.push(b'\n');
            acc.push(v);
        }

        acc.into_iter().flatten().collect::<Vec<u8>>()
    }

    fn handle_input(line: String, cmd_tx: &MSender<InputCmd>) -> bool {
        let mut quit = false;

        info!("Received a new line: {:?}", &line);

        // handles terminal input switching
        match line.as_str() {
            "\\sessions" | "\\ss" => {
                info!("Sessions request...");
                cmd_tx.blocking_send(InputCmd::Sessions).unwrap();
            },
            value if value.starts_with("\\switch") || value.starts_with("\\sw") => {
                if let Some(id_str) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let switch_id_str = id_str.to_owned(); // &str to String
                    let switch_id = switch_id_str.parse::<u16>().unwrap_or_default() + IO_ID_OFFSET; // String to u16
                    info!("Attempting to switch input to {}", &switch_id);
                    cmd_tx.blocking_send(InputCmd::Switch(switch_id)).unwrap();
                }
            },
            "\\quit" => {
                info!("Input handler received quit.. aborting but passing on first");
                quit = true;
                cmd_tx.blocking_send(InputCmd::Quit).unwrap();
            },
            _ => {
                cmd_tx.blocking_send(InputCmd::Other(line)).unwrap();
            }
        }

        quit
    }

    // spawn thread and tokio task
    pub fn spawn(mut input: InputHandler) -> (std::thread::JoinHandle<()>, tokio::task::JoinHandle<()>) {
        let shared = input.shared.clone();
        let watch_tx = input.watch_tx.take().unwrap();
        let mut msg_rx = input.msg_rx.take().unwrap();

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<InputCmd>(1);

        let mut current_id = IO_ID_START;
        let mut seq_no = 0;

        // single thread dedicated to std input reads
        // it communicates with the tokio task via cmd_tx
        let thread_handle = thread::spawn(move || {
            let stdin = stdio::stdin();
            let lines = stdin.lock().lines();

            for line in lines.map(|l| l.unwrap()) {
                if Self::handle_input(line, &cmd_tx) {
                    info!("Received quit terminating input thread");
                    return
                }
            }

        });

        // spawn tokio task for input cmd receives and input msg receives
        let task_handle = tokio::spawn(async move {
            let mut quit = false;

            loop {
                select!(
                    Some(cmd) = cmd_rx.recv(), if !quit => {
                        info!("Received a new line: {:?}", &cmd);

                        match cmd {
                            InputCmd::Sessions => {
                                info!("!! Sessions request...");
                                shared.display_sessions(current_id).await;
                                continue;  // don't forward the sessions msg on
                            },
                            InputCmd::Switch(switch_id) => {
                                    // Local validation for valid id
                                if shared.contains_id(&switch_id).await {
                                    info!("!! Successful id and line switch, old id {} new id {}", current_id, switch_id);
                                    // swap out old current id and line and replace with new
                                    current_id = switch_id;
                                    shared.switch_id(switch_id).await;
                                } else { // if switch_id not valid ignore and continue
                                    info!("!! not a valid switch id");
                                }
                                continue; // don't forward switch message
                            },
                            InputCmd::Quit => {
                                info!("!! Input handler received quit.. aborting but passing on first");
                                quit = true;
                                shared.switch_line("\\quit".to_owned()).await;
                            },
                            InputCmd::Other(ref line) => {
                                shared.switch_line(line.clone()).await; // replace old line with new line
                            },
                        }

                        seq_no += 1;

                        info!("Sending new change seq_no {}, current_id {} corresponding to cmd {:?}", seq_no, current_id, &cmd);

                        // notify that a new line has been received (changing seq no) along with the current id
                        watch_tx.send((seq_no, current_id)).expect("Unable to send value on watch channel");
                    }
                    Some(msg) = msg_rx.recv() => {
                        info!("Received a new input msg: {:?}", &msg);

                        match msg {
                            InputMsg::NewSession(id, name) => {
                                if shared.new_session(id, name).await {
                                    current_id = id;
                                }
                            },
                            InputMsg::UpdatedSessionName(id, name) => {
                                shared.update_session_name(id, name).await;
                            },
                            InputMsg::CloseSession(id) => {
                                if shared.close_session(id).await {
                                    current_id = IO_ID_START;
                                }
                            },
                            InputMsg::CloseLobby => {
                                shared.close_lobby().await;
                            }
                        }

                        if quit {
                            return
                        }
                    }
                )
            }
        });

        (thread_handle, task_handle)
    }

    // blocking function to gather user input from std::io::stdin
    // should be called first for name registration
    pub fn read_sync_user_input(prompt: &str) -> io::Result<Option<Vec<u8>>> {
        let mut buf = String::new();

        print!("{} ", prompt);
        stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
        stdio::stdin().read_line(&mut buf).expect("unable to read command line input");
        let name = buf.trim_end().as_bytes().to_owned();

        Ok(Some(name))
    }

    pub async fn read_async_user_input(current_id: u16,
                                       input_rx: &mut InputReceiver,
                                       io_shared: &InputShared)
                                       -> io::Result<Option<String>> {
        let new_line: bool;
        let watch_id: u16;
        let seq_id: u16;

        {
            // keep borrowed scope small
            new_line = input_rx.changed().await.is_ok();
            seq_id = input_rx.borrow().0.clone();
            watch_id = input_rx.borrow().1.clone();
        }

        info!("read_async await ok status: new_line {} seq_id {} watch_id {}", new_line, seq_id, watch_id);

        if new_line {
            if current_id == watch_id {
                if let Some(line) = io_shared.get_line_if_id_matches(current_id).await {
                    return Ok(Some(line));
                }
                else {
                    info!("checkpoint A.2 current id didn't match arc current rwlock hence no line to provide");
                    return Ok(None);
                }
            } else {
                info!("current_id {} doesn't match watch_id {} from watch channel", current_id, watch_id);
                return Ok(None);
            }
        }

        info!("checkpoint B.2 no new line received");

        return Err(Error::new(ErrorKind::Other, "no new lines as input_tx has been dropped"));
    }
}

pub struct InputBase {
    current: IdLinePair,
    sessions: Sessions,
    rx: InputReceiver,
    tx: InputNotifier,
    counter: AtomicU16,
}

pub struct InputShared {
    shared: Arc<InputBase>
}

impl InputShared {
    pub fn new(seed_id: u16, rx: InputReceiver, tx: InputNotifier) -> Self {
        let current = RwLock::new((seed_id, String::from("empty")));
        let sessions = RwLock::new(HashMap::from([(seed_id, String::from("main lobby"))]));
        let counter = AtomicU16::new(seed_id);

        InputShared{
            shared: Arc::new(
                InputBase {
                    current,
                    sessions,
                    rx,
                    tx,
                    counter
                }
            )
        }
    }

    pub(crate) async fn notify(&self, msg: InputMsg) -> Result<(), SendError<InputMsg>> {
        self.shared.tx.send(msg).await
    }

    pub(crate) fn get_notifier(&self) -> InputNotifier {
        self.shared.tx.clone()
    }

    pub(crate) fn get_receiver(&self) -> InputReceiver {
        self.shared.rx.clone()
    }

    pub(crate) async fn get_next_id(&self) -> u16 {
        let new_id = self.shared.counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client
        if new_id != IO_ID_START {
            self.shared.sessions.write().await.insert(new_id, String::new());     // add to RwLock
        }
        new_id
    }

    async fn new_session(&self, io_id: u16, name: String) -> bool {
        self.force_switch_id(io_id).await &&
            self.shared.sessions.write().await.insert(io_id, name).is_some()
    }

    async fn update_session_name(&self, io_id: u16, name: String) -> bool {
        self.shared.sessions.write().await.insert(io_id, name).is_some()
    }

    async fn close_session(&self, io_id: u16) -> bool {
        self.force_switch_id(IO_ID_START).await &&
            self.remove_id(io_id).await
    }

    async fn close_lobby(&self) -> bool {
        self.remove_id(IO_ID_START).await
    }

    // remove from RwLock
    async fn remove_id(&self, id: u16) -> bool {
        self.shared.sessions.write().await.remove(&id).is_some()
    }

    async fn contains_id(&self, id: &u16) -> bool {
        self.shared.sessions.read().await.contains_key(id)
    }

    async fn force_switch_id(&self, switch_id: u16) -> bool {
        let mut switched: bool = false;

        if self.contains_id(&switch_id).await {
            // set lock inner value to id
            self.switch_id(switch_id).await;
            switched = true;
        }

        {
            let inner = self.shared.current.read().await;
            println!("sanity check did we update shared current id? {} {}", inner.0, switch_id);
        }
        if switched {
            if switch_id == IO_ID_START {
                println!(">>> Auto switched back to main lobby {}", switch_id - IO_ID_OFFSET);
            } else {
                println!(">>> Auto switched to session {}, to switch back to main lobby, type: \\sw 0", switch_id - IO_ID_OFFSET);
            }
        }

        switched
    }

    async fn switch_id(&self, switch_id: u16) {
        // set lock inner value to id
        let mut inner  = self.shared.current.write().await;
        inner.0 = switch_id;
    }

    /* Current id and line methods */
    async fn switch_line(&self, line: String) {
        // set lock inner value to new line
        let mut inner  = self.shared.current.write().await;
        *inner = (inner.0, line);
    }

    async fn get_line_if_id_matches(&self, match_id: u16) -> Option<String> {
        let (id, s) = &*self.shared.current.read().await;

        if match_id == *id {
            Some(s.clone())
        } else {
            None
        }
    }

    async fn display_sessions(&self, current_id: u16) {
        //let map_iter = self.shared.sessions.read().await.iter();
        let mut prefix: &str = " ";
        println!("<Active Sessions>");

        for (k, v) in self.shared.sessions.read().await.iter() {

            // if current session, mark with *
            if *k == current_id {
                prefix = "*";
            }

            if *k == IO_ID_START {
                println!("{}{}, broadcast chat in {}", prefix, *k - IO_ID_OFFSET, v);
            } else {
                println!("{}{}, peer chat --> {}", prefix, *k - IO_ID_OFFSET, v);
            }

            prefix = " ";
        }
    }

    // Create another InputShared struct with the shared field Arc cloned
    pub fn clone(&self) -> Self {
        InputShared {
            shared: Arc::clone(&self.shared),
        }
    }
}
