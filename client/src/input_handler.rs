use std::{thread, time};
use std::io as stdio;
use std::io::{stdout, Write};
use std::io::BufRead;
use std::collections::HashMap;

use tokio::io::{self, Error, ErrorKind};
use tokio::time::{sleep, Duration};
use tokio::select;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender as MSender;
use tokio::sync::mpsc::Receiver as MReceiver;
use tokio::sync::watch::{self, Sender, Receiver};

use tracing::info;

pub type InputNotifier = MSender<InputMsg>;
pub type InputReceiver = Receiver<(u16, u16)>;
type InputSender = Sender<(u16, u16)>;

pub const IO_ID_OFFSET: u16 = 1000;
const IO_ID_LOBBY: u16 = 1000;
const USER_LINES: usize = 64;
const QUIT_STR: &str = "\\quit";

pub type IdLinePair = RwLock<(u16, String)>; // contains the io id along with the current line
pub type Sessions = RwLock<HashMap<u16, String>>;

use crate::input_shared::InputShared;

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
        let shared = InputShared::new(IO_ID_LOBBY, watch_rx, msg_tx);

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

        let mut current_id = IO_ID_LOBBY;
        let mut seq_no = 0;

        // single thread dedicated to std input reads
        // it communicates with the tokio task via cmd_tx
        let thread_handle = thread::spawn(move || {
            let stdin = stdio::stdin();
            let lines = stdin.lock().lines();

            for line in lines.map(|l| l.unwrap()) {
                if Self::handle_input(line, &cmd_tx) {
                    info!("Received quit, terminating input thread");
                    thread::sleep(time::Duration::from_millis(500));
                    break;
                }
            }
        });

        // spawn tokio task for input cmd receives and input msg receives
        let task_handle = tokio::spawn(async move {
            let mut quit = false;

            loop {
                select!(
                    Some(cmd) = cmd_rx.recv(), if !quit => {
                        info!("Received a new cmd: {:?}", &cmd);

                        match cmd {
                            InputCmd::Sessions => {
                                info!("!! Sessions request...");
                                shared.display_sessions(current_id).await;
                                continue;  // don't forward the sessions msg on
                            },
                            InputCmd::Switch(switch_id) => {
                                if shared.switch_id(switch_id).await {
                                    info!("!! Successful id and line switch, old id {} new id {}", current_id, switch_id);
                                    // swap out old current id and line and replace with new
                                    current_id = switch_id;
                                } else { // if switch_id not valid ignore and continue
                                    info!("!! not a valid switch id");
                                }
                                continue; // don't forward switch message
                            },
                            InputCmd::Quit => {
                                info!("!! 1 Input handler received quit.. aborting but passing on first");
                                shared.switch_line(QUIT_STR.to_owned()).await;
                                Self::watch_send(&mut seq_no, current_id, &cmd, &watch_tx).await;

                                // if peer client session is active,
                                // send quit message also to main lobby session (if available)
                                // hence switch over to lobby session and fire same message, if not, no issue
                                if current_id != IO_ID_LOBBY {
                                    sleep(Duration::from_millis(100)).await; // wait a little for prev msg to settle
                                    if shared.switch_id_line(IO_ID_LOBBY, QUIT_STR.to_owned()).await {
                                        current_id = IO_ID_LOBBY;
                                    }
                                }
                                quit = true;
                            },
                            InputCmd::Other(ref line) => {
                                shared.switch_line(line.clone()).await; // replace old line with new line
                            },
                        }

                        Self::watch_send(&mut seq_no, current_id, &cmd, &watch_tx).await;
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
                                    current_id = IO_ID_LOBBY;
                                }
                            },
                            InputMsg::CloseLobby => {
                                shared.close_lobby().await;
                            }
                        }

                        // Quit not waiting for the other side to finish
                        if quit {
                            info!("X Terminating tokio input handler task");
                            break;
                        }
                    }
                )
            }
        });

        (thread_handle, task_handle)
    }

    async fn watch_send(uid: &mut u16, sid: u16, cmd: &InputCmd, tx: &InputSender) {
        *uid += 1;
        info!("Sending new change uid {}, session_id {} corresponding to cmd {:?}", uid, sid, cmd);
        // notify that a new line has been received (changing seq no) along with the current id
        tx.send((*uid, sid)).expect("Unable to send value on watch channel");
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
