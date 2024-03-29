//! Shared abstraction to help manage io active session information
//! Creating new io_id values, displaying and changing the current active io_id value,
//! Removing io_id information, along with managing active current line data

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::collections::HashMap;
use tokio::sync::RwLock;
//use tracing::info;

use crate::input_reader::{session_id, is_lobby, lobby_id};

type InputReceiver = crate::input_handler::InputReceiver;
type IdLinePair = RwLock<(u16, String)>; // contains the io id along with the current line
type Sessions = RwLock<HashMap<u16, String>>;

pub struct InputBase {
    current: IdLinePair,
    sessions: Sessions,
    rx: InputReceiver,
    counter: AtomicU16,
}

pub struct InputShared {
    shared: Arc<InputBase>
}

impl InputShared {
    pub fn new(seed_id: u16, rx: InputReceiver) -> Self {
        let current = RwLock::new((seed_id, String::from("empty")));
        let sessions = RwLock::new(HashMap::from([(seed_id, String::from("main lobby"))]));
        let counter = AtomicU16::new(seed_id);

        InputShared{
            shared: Arc::new(
                InputBase {
                    current,
                    sessions,
                    rx,
                    counter
                }
            )
        }
    }

    pub(crate) fn get_receiver(&self) -> InputReceiver {
        self.shared.rx.clone()
    }

    pub(crate) async fn get_next_id(&self) -> u16 {
        let new_id = self.shared.counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client
        if !is_lobby(new_id) {
            self.shared.sessions.write().await.insert(new_id, String::new());     // add to RwLock
        }
        new_id
    }

    pub(crate) async fn new_session(&self, io_id: u16, name: String) -> bool {
        self.display_and_switch_id(io_id).await &&
            self.shared.sessions.write().await.insert(io_id, name).is_some()
    }

    pub(crate) async fn update_session_name(&self, io_id: u16, name: String) -> bool {
        self.shared.sessions.write().await.insert(io_id, name).is_some()
    }

    pub(crate) async fn close_session(&self, io_id: u16) -> bool {
        self.display_and_switch_id(lobby_id()).await &&
            self.remove_id(io_id).await
    }

    pub(crate) async fn close_lobby(&self) -> bool {
        self.remove_id(lobby_id()).await
    }

    // remove from RwLock
    async fn remove_id(&self, id: u16) -> bool {
        self.shared.sessions.write().await.remove(&id).is_some()
    }

    async fn contains_id(&self, id: &u16) -> bool {
        self.shared.sessions.read().await.contains_key(id)
    }

    async fn display_and_switch_id(&self, switch_id: u16) -> bool {
        let switched = self.switch_id(switch_id).await; // set lock inner value to id

        if switched {
            if is_lobby(switch_id) {
                println!(">>> Auto switched back to main lobby {}", session_id(switch_id));
            } else {
                println!(">>> Auto switched to session {}, for lobby do: \\lob", session_id(switch_id));
            }
        }

        switched
    }

    pub(crate) async fn switch_id(&self, switch_id: u16) -> bool {
        let mut switched: bool = false;

        if self.contains_id(&switch_id).await {
            // set lock inner value to id
            let mut inner  = self.shared.current.write().await;
            inner.0 = switch_id;
            switched = true;
        }

        switched
    }

    pub(crate) async fn switch_id_line(&self, switch_id: u16, line: String) -> bool {
        let mut switched = false;

        if self.contains_id(&switch_id).await {
            // set lock inner value to id and line
            let mut inner  = self.shared.current.write().await;
            *inner = (switch_id, line);
            switched = true;
        }

        switched
    }

    /* Current id and line methods */
    pub(crate) async fn switch_line(&self, line: String) {
        // set lock inner value to new line
        let mut inner  = self.shared.current.write().await;
        *inner = (inner.0, line);
    }

    pub(crate) async fn get_line_if_id_matches(&self, match_id: u16) -> Option<String> {
        let (id, s) = &*self.shared.current.read().await;

        if match_id == *id {
            Some(s.clone())
        } else {
            None
        }
    }

    pub(crate) async fn display_sessions(&self, current_id: u16) {
        let mut prefix: &str = " ";
        println!("< Active Sessions (type \\sw (session id) to switch over) >");

        for (k, v) in self.shared.sessions.read().await.iter() {

            // if current session, mark with *
            if *k == current_id {
                prefix = "*";
            }

            if is_lobby(*k) {
                println!("{}{}, broadcast chat in {}", prefix, session_id(*k), v);
            } else {
                println!("{}{}, peer chat --> {}", prefix, session_id(*k), v);
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
