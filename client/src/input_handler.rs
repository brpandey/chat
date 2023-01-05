//use std::thread;
use std::io as stdio;
use std::io::{stdout, Write};

use std::sync::{Arc, /*Mutex, RwLock */};
use std::sync::atomic::{AtomicU16, Ordering};
use std::collections::HashSet;

use tokio::sync::RwLock;
use tokio::sync::watch::{self, Sender, Receiver};
use tokio::io;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_stream::StreamExt; // provides combinator methods like next on to of FramedRead buf read and Stream trait

use tracing::info;

pub type InputReceiver = Receiver<(u16, u16)>;
type InputSender = Sender<(u16, u16)>;

pub const IO_ID_OFFSET: u16 = 1000;
const IO_ID_START: u16 = 1000;
const LINES_MAX_LEN: usize = 256;
const USER_LINES: usize = 64;

pub type IdLinePair = RwLock<(u16, String)>; // contains the io id along with the current line
pub type IoIds = RwLock<HashSet<u16>>;

pub struct InputHandler {
    shared: InputShared,
    tx: Option<InputSender>,
}

impl InputHandler {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel((0, 0)); // Watch channel has 1 tx and potentially multiple receivers
        let shared = InputShared::new(IO_ID_START, rx);

        Self {
            shared,
            tx: Some(tx),
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

    pub fn spawn(mut input: InputHandler) -> tokio::task::JoinHandle<()> {
        let shared = input.shared.clone();
        let tx = input.tx.take().unwrap();
        let mut current_id = IO_ID_START;
        let mut seq_no = 0;

        tokio::spawn(async move {

            loop {
                let mut fr = FramedRead::new(tokio::io::stdin(), LinesCodec::new_with_max_length(LINES_MAX_LEN));
                //            let stdin = io::stdin();

                if let Some(Ok(line)) = fr.next().await {

                    // grab the stdin lines at the exclusive behest of anyone else
                    //            for line in stdin.lock().lines().map(|l| l.unwrap()) {

                    // handles terminal input switching
                    if line.starts_with("\\switch") {
                        if let Some(id_str) = line.splitn(3, ' ').skip(1).take(1).next() {
                            let switch_id_str = id_str.to_owned(); // &str to String
                            let switch_id = switch_id_str.parse::<u16>().unwrap_or_default() + IO_ID_OFFSET; // String to u16
                            info!("Attempting to switch input to {}", &switch_id);

                            // Local validation for valid id
                            if shared.contains_id(&switch_id).await {
                                info!("Successful id and line switch, old id {} new id {}", current_id, switch_id);
                                // swap out old current id and line and replace with new
                                current_id = switch_id;
                                shared.switch_id_and_line(switch_id, &line).await;
                            } else { // if switch_id not valid ignore and continue
                                info!("not a valid switch id");
                                continue;
                            }
                        }
                    } else {
                        shared.switch_line(&line).await; // replace old line with new line
                    }

                    seq_no += 1;

                    info!("Sending new change seq_no {}, current_id {} corresponding to line {}", seq_no, current_id, &line);

                    // notify that a new line has been received (changing seq no) along with the current id
                    tx.send((seq_no, current_id)).expect("Unable to send value on watch channel");
                }
            }
        })
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

            info!("read_async await ok status: new_line {} seq_id {} watch_id {}", new_line, seq_id, watch_id);
        }

        if new_line {
            if current_id == watch_id {
                if let Some(line) = io_shared.get_line_if_id_matches(current_id).await {
                    return Ok(Some(line));
                }
                else {
                    info!("checkpoint A.2 current id didn't match arc current rwlock hence no line to provide");
                }
            } else {
                info!("current_id doesn't match watch_id from watch channel");
            }
        }
        else {
            info!("checkpoint B.2 no new line received ");
        }

        return Ok(None)
    }
}

pub struct InputBase {
    current: IdLinePair,
    ids: IoIds,
    rx: InputReceiver,
    counter: AtomicU16,
}

pub struct InputShared {
    shared: Arc<InputBase>
}

impl InputShared {
    pub fn new(seed_id: u16, rx: InputReceiver) -> Self {
        let current = RwLock::new((seed_id, String::from("empty")));
        let ids = RwLock::new(HashSet::from([seed_id]));
        let counter = AtomicU16::new(seed_id);

        InputShared{
            shared: Arc::new(
                InputBase {
                    current,
                    ids,
                    rx,
                    counter
                }
            )
        }
    }

    pub fn get_receiver(&self) -> InputReceiver {
        self.shared.rx.clone()
    }

    /* Id methods */
    pub async fn get_next_id(&self) -> u16 {
        let new_id = self.shared.counter.fetch_add(1, Ordering::Relaxed); // establish unique id for client
        self.shared.ids.write().await.insert(new_id);     // add to RwLock
        new_id
    }

    // remove from RwLock
    pub async fn remove_id(&self, id: u16) {
        self.shared.ids.write().await.remove(&id);
    }

    async fn contains_id(&self, id: &u16) -> bool {
        self.shared.ids.read().await.contains(id)
    }


    /* Current id and line methods */
/*    pub async fn switch_line(&self, line: &str) {
        // set lock inner value to new line
        let mut inner  = self.shared.current.write().await;
        *inner = (inner.0, line.to_owned());
    }
*/

    /* Current id and line methods */
    async fn switch_line(&self, line: &str) {
        // set lock inner value to new line
        let mut inner  = self.shared.current.write().await;
        *inner = (inner.0, line.to_owned());
    }

    async fn switch_id_and_line(&self, switch_id: u16, line: &str) {
        // set lock inner value to new io id along with new line
        let mut inner = self.shared.current.write().await;
        *inner = (switch_id, line.to_owned());
    }

    async fn get_line_if_id_matches(&self, match_id: u16) -> Option<String> {
        let (id, s) = &*self.shared.current.read().await;

        if match_id == *id {
            Some(s.clone())
        } else {
            None
        }
    }

    // Create another InputShared struct with the shared field Arc cloned
    pub fn clone(&self) -> Self {
        InputShared {
            shared: Arc::clone(&self.shared),
        }
    }
}
