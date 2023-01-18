use std::io as stdio;
use std::io::{stdout, Write};
use tokio::io::{self, Error, ErrorKind};
use tracing::info;

use crate::input_shared::InputShared;
use crate::input_handler::InputReceiver;

pub const IO_ID_OFFSET: u16 = crate::input_handler::IO_ID_LOBBY;
const USER_LINES: usize = 64;

pub struct InputReader;

impl InputReader {

    pub fn session_id(io_id: u16) -> u16 {
        io_id - IO_ID_OFFSET
    }

    pub fn io_id(session_id: u16) -> u16 {
        session_id + IO_ID_OFFSET
    }

    // blocking function to gather user input from std::io::stdin
    // should be called first for name registration
    pub fn blocking_read(prompt: &str) -> io::Result<Option<Vec<u8>>> {
        let mut buf = String::new();

        print!("{} ", prompt);
        stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
        stdio::stdin().read_line(&mut buf).expect("unable to read command line input");
        let name = buf.trim_end().as_bytes().to_owned();

        Ok(Some(name))
    }

    pub async fn read(current_id: u16,
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
}
