//! Provides central place to handle standard input cmd line reads
//! Notifies other tokio tasks through a watch channel two things
//! 1) when a new line is received
//! 2) what io_id / session_id is current subscribed to reading input
//!
//! The input handler defines control messages to view and change the current io session info
//! Clients and peer clients are able to communicate through msg passing to notify of
//! auto switching to certain default sessions e.g. lobby or a new peer session

//! Works in conjunction with input shared and input reader

use std::{io as stdio, io::BufRead, thread, time};
use tokio::{select, time::sleep, time::Duration};
use tokio::sync::mpsc::{self, Sender as MSender, Receiver as MReceiver};
use tokio::sync::watch::{self, Sender as WSender, Receiver as WReceiver};
use tracing::{debug, error};

pub type InputNotifier = MSender<InputMsg>;
pub type InputReceiver = WReceiver<(u16, u16)>;
type InputSender = WSender<(u16, u16)>;

pub const IO_ID_LOBBY: u16 = 1000;
const QUIT_STR: &str = "\\quit";

use crate::types::InputMsg;
use crate::input_shared::InputShared;
use crate::input_reader::io_id;

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
    msg_tx: MSender<InputMsg>,
}

impl InputHandler {
    pub fn new() -> Self {
        let (msg_tx, msg_rx) = mpsc::channel::<InputMsg>(16);
        let (watch_tx, watch_rx) = watch::channel((0, 0)); // Watch channel has 1 tx and potentially multiple receivers
        let shared = InputShared::new(IO_ID_LOBBY, watch_rx);

        Self {
            shared,
            watch_tx: Some(watch_tx),
            msg_rx: Some(msg_rx),
            msg_tx,
        }
    }

    pub fn get_shared(&self) -> InputShared {
        self.shared.clone()
    }

    pub fn get_notifier(&self) -> InputNotifier {
        self.msg_tx.clone()
    }

    // convert user text cmds into channel msgs that are sent from
    // an input thread to a tokio task
    fn parse_cmdline_input(line: String, cmd_tx: &MSender<InputCmd>) -> bool {
        let mut quit = false;

        debug!("Received a new line: {:?}", &line);

        // handles terminal input switching
        match line.as_str() {
            "\\lobby" | "\\lob" => {
                cmd_tx.blocking_send(InputCmd::Switch(IO_ID_LOBBY)).unwrap();
            },
            "\\sessions" | "\\ss" => {
                cmd_tx.blocking_send(InputCmd::Sessions).unwrap();
            },
            value if value.starts_with("\\switch") || value.starts_with("\\sw") => {
                if let Some(id_str) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let switch_id_str = id_str.to_owned(); // &str to String
                    let switch_id = io_id(switch_id_str.parse::<u16>().unwrap_or_default()); // String to u16
                    cmd_tx.blocking_send(InputCmd::Switch(switch_id)).unwrap();
                }
            },
            "\\quit" => {
                debug!("Input handler received quit.. aborting but passing on first");
                quit = true;
                cmd_tx.blocking_send(InputCmd::Quit).unwrap();
            },
            _ => {
                cmd_tx.blocking_send(InputCmd::Other(line)).unwrap();
            }
        }

        quit
    }

    // handle two types of incoming messages
    // 1) cmds (and text) received from cmdline channel and, if applicable, along to the watch channel
    // 2) input msgs from other parts of the application
    async fn handle_incoming_msgs(mut input: InputHandler, mut cmd_rx: MReceiver<InputCmd>) {
        let mut quit = false;
        let mut current_id = IO_ID_LOBBY;
        let mut seq_no = 0;

        let shared = input.shared.clone();
        let watch_tx = input.watch_tx.take().unwrap();
        let mut msg_rx = input.msg_rx.take().unwrap();

        loop {
            select!(
                Some(cmd) = cmd_rx.recv(), if !quit => {
                    debug!("Received a new line or cmd: {:?}", &cmd);

                    match cmd {
                        InputCmd::Sessions => {
                            shared.display_sessions(current_id).await;
                            continue;  // don't forward the sessions msg on
                        },
                        InputCmd::Switch(switch_id) => {
                            if shared.switch_id(switch_id).await {
                                // swap out old current id and line and replace with new
                                current_id = switch_id;
                            } else { // if switch_id not valid ignore and continue
                                error!("User specified an invalid switch id");
                            }
                            continue; // don't forward switch message
                        },
                        InputCmd::Quit => {
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
                    debug!("Received a new input msg: {:?}", &msg);

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
                        sleep(Duration::from_millis(2000)).await;
                        debug!("terminating tokio input handler task");
                        break;
                    }
                }
            )
        }
    }

    // spawn both input thread and tokio cmd handler task
    pub fn spawn(input: InputHandler) -> (std::thread::JoinHandle<()>, tokio::task::JoinHandle<()>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<InputCmd>(1);

        // single thread dedicated to std input reads
        // it communicates with the tokio task via cmd_tx
        let thread_handle = thread::spawn(move || {
            let stdin = stdio::stdin();
            let lines = stdin.lock().lines();

            for line in lines.map(|l| l.unwrap()) {
                if Self::parse_cmdline_input(line, &cmd_tx) {
                    thread::sleep(time::Duration::from_millis(500));
                    debug!("Received quit, terminating input thread");
                    break;
                }
            }
        });

        // spawn tokio task for input cmd receives and input msg receives
        let task_handle = tokio::spawn(async move {
            Self::handle_incoming_msgs(input, cmd_rx).await;
        });

        (thread_handle, task_handle)
    }

    // helper function to send on watch channel notifying input consumers (client and peer clients)
    async fn watch_send(uid: &mut u16, sid: u16, cmd: &InputCmd, tx: &InputSender) {
        *uid += 1;
        debug!("Sending new change uid {}, session_id {} corresponding to cmd {:?}", uid, sid, cmd);
        // notify that a new line has been received (changing seq no) along with the current id
        tx.send((*uid, sid)).expect("Unable to send value on watch channel");
    }
}
