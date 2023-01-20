use std::{io as stdio, io::BufRead, thread, time};
use tokio::{select, time::sleep, time::Duration};
use tokio::sync::mpsc::{self, Sender as MSender, Receiver as MReceiver};
use tokio::sync::watch::{self, Sender as WSender, Receiver as WReceiver};
use tracing::info;

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

    fn handle_input(line: String, cmd_tx: &MSender<InputCmd>) -> bool {
        let mut quit = false;

        info!("Received a new line: {:?}", &line);

        // handles terminal input switching
        match line.as_str() {
            "\\lobby" | "\\lob" => {
                cmd_tx.blocking_send(InputCmd::Switch(IO_ID_LOBBY)).unwrap();
            },
            "\\sessions" | "\\ss" => {
                info!("Sessions request...");
                cmd_tx.blocking_send(InputCmd::Sessions).unwrap();
            },
            value if value.starts_with("\\switch") || value.starts_with("\\sw") => {
                if let Some(id_str) = value.splitn(3, ' ').skip(1).take(1).next() {
                    let switch_id_str = id_str.to_owned(); // &str to String
                    let switch_id = io_id(switch_id_str.parse::<u16>().unwrap_or_default()); // String to u16
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
                            sleep(Duration::from_millis(2000)).await;
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
}


