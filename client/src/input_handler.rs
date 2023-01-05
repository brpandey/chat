//use std::thread;
use std::sync::{Arc, /*Mutex,*/ RwLock};
use std::collections::HashSet;
//use std::io::{self, BufRead};
use std::sync::atomic::{AtomicU16, Ordering};

// getting it working first with broadcast and consider watch later
use tokio::sync::broadcast::{self, Sender, Receiver};

//use tokio::sync::watch::{self, Sender, Receiver};
//use tokio::io;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait

use tracing::info;

type InputSender = Sender<(u16, String)>;
pub type InputReceiver = Receiver<(u16, String)>;

const LINES_MAX_LEN: usize = 256;
const USER_LINES: usize = 64;

const IO_ID_OFFSET: u16 = 1000;
pub const IO_ID_START: u16 = 1000;

// contains the turn id along with the current line
//pub type TurnLinePair = Mutex<(u16, String)>;
pub type IoIds = RwLock<HashSet<u16>>;

pub struct InputHandler {
    shared: InputShared,
    //    tx: Option<InputSender>,
    tx: InputSender,
    rx: InputReceiver,
}

impl InputHandler {
    pub fn new() -> Self {
        let (tx, rx) = broadcast::channel(16);
//        let (tx, rx) = watch::channel((0, String::new())); // Watch channel has 1 tx and potentially multiple receivers
        let shared = InputShared::new(IO_ID_START, tx.clone());

        Self {
            shared,
            tx,
            rx,
        }
    }

    pub fn get_shared(&self) -> InputShared {
        self.shared.clone()
    }

    pub fn interleave_newlines(lines: &str, mut acc: Vec<Vec<u8>>) -> Vec<u8> {
        let mut v: Vec<u8>;

        while let Some(chunk) = lines.as_bytes().chunks(USER_LINES).next() {
            v = chunk.to_vec();
            v.push(b'\n');
            acc.push(v);
        }

        acc.into_iter().flatten().collect::<Vec<u8>>()
    }

    pub fn spawn(input: InputHandler) -> tokio::task::JoinHandle<()> {
        let shared = input.shared.clone();
        //        let tx = input.tx.take().unwrap();
        let tx = input.tx.clone();
        let mut current_id = IO_ID_START; // shared.ids.read().unwrap().iter().cloned().take(1).unwrap();

        tokio::spawn(async move {

            let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new_with_max_length(LINES_MAX_LEN));
//            let stdin = io::stdin();

            if let Some(Ok(line)) = fr.next().await {

            // grab the stdin lines at the exclusive behest of anyone else
//            for line in stdin.lock().lines().map(|l| l.unwrap()) {
                println!("echo {}", &line);

                // handles terminal input switching
                if line.starts_with("\\switch") {
                    if let Some(id_str) = line.splitn(3, ' ').skip(1).take(1).next() {
                        let switch_id_str = id_str.to_owned(); // &str to String
                        let switch_id = switch_id_str.parse::<u16>().unwrap_or_default() + IO_ID_OFFSET; // String to u16
                        info!("Attempting to switch input to {}", &switch_id);

                        if shared.contains_id(&switch_id) { // Local validation if switch_id is present
                            {
                                // mutex now contains value of input id to switch to
                                // along with current line
  //                              (id, s) = handler.current.lock().unwrap();
  //                              *id = switch_id;
                                current_id = switch_id;
  //                              *s = line.clone();
                            }
                        }
                    }
                } /* else {
                    {
                        // mutex contains value of updated current line from stdin
  //                      (_, s) = handler.current.lock().unwrap();
  //                      *s = line.clone();
                    }
                } */

                tx.send((current_id, line)).expect("Unable to send value on watch channel");
            }
        })
    }
}

pub struct InputBase {
    //    current: TurnLinePair, // don't even really need since we are sending data on watch channel TODO - REMOVE
    ids: IoIds,
    //    rx: InputReceiver,
    tx: InputSender,
    counter: AtomicU16,
}

pub struct InputShared {
    shared: Arc<InputBase>
}

impl InputShared {
    pub fn new(seed_id: u16, tx: InputSender) -> Self {
//    pub fn new(seed_id: u16, rx: InputReceiver) -> Self {
//        let current = Mutex::new((turn_id, String::new())); // current turn is main client or 1000
        let ids = RwLock::new(HashSet::from([seed_id]));
        let counter = AtomicU16::new(seed_id);

        InputShared{
            shared: Arc::new(
                InputBase {//            current,
                    ids,
                    //                    rx,
                    tx,
                    counter
                }
            )
        }
    }

    pub fn get_receiver(&self) -> InputReceiver {
        self.shared.tx.subscribe()
//        self.shared.rx.clone()
    }

    pub fn contains_id(&self, id: &u16) -> bool {
        self.shared.ids.read().unwrap().contains(id)
    }

    pub fn get_next_id(&self) -> u16 {
        let new_id = self.shared.counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client
        self.shared.ids.write().unwrap().insert(new_id);     // add to RwLock
        new_id
    }

    // remove from RwLock
    pub fn remove_id(&self, id: u16) {
        self.shared.ids.write().unwrap().remove(&id);
    }

    // Create another InputShared struct with the shared field Arc cloned
    pub fn clone(&self) -> Self {
        InputShared {
            shared: Arc::clone(&self.shared),
        }
    }

}
