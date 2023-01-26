//! Abstraction for input consumers to read stdin data
//! relevant to each specific consumer io_id.

//! For a given client or peer client, handles the locking check
//! if a new line event is suited for the client and returns the new line

//! Provides convenience functions to read stdin both blocking and non-blocking
//! With helpers around translating io_ids to session_ids and
//! interleaving in newlines for multi-line input

use std::io as stdio;
use std::io::{stdout, Write};
use tokio::io;
use tracing::{info, debug};

use crate::input_shared::InputShared;
use crate::input_handler::InputReceiver;
use crate::input_handler::IO_ID_LOBBY;
use crate::types::ReaderError;

pub const IO_ID_OFFSET: u16 = IO_ID_LOBBY;
const USER_LINES: usize = 64;

pub fn lobby_id() -> u16 {
    IO_ID_LOBBY
}

pub fn is_lobby(io_id: u16) -> bool {
    io_id == IO_ID_OFFSET
}

pub fn session_id(io_id: u16) -> u16 {
    io_id - IO_ID_OFFSET
}

pub fn io_id(session_id: u16) -> u16 {
    session_id + IO_ID_OFFSET
}


pub struct InputReader;

impl InputReader {

    // blocking function to gather user input from std::io::stdin
    // should be called first for name registration
    pub fn blocking_read(prompt: &str) -> io::Result<Option<Vec<u8>>> {
        let mut buf = String::new();

        print!("{} ", prompt);
        stdout().flush()?;  // Since stdout is line buffered need to explicitly flush
        stdio::stdin().read_line(&mut buf)?;
        let name = buf.trim_end().as_bytes().to_owned();

        Ok(Some(name))
    }

    pub async fn read(current_id: u16,
                      input_rx: &mut InputReceiver,
                      io_shared: &InputShared)
                      -> Result<Option<String>, ReaderError> {
        let new_line: bool;
        let watch_id: u16;
        let seq_id: u16;

        {
            // keep borrowed scope small
            new_line = input_rx.changed().await.is_ok();
            seq_id = input_rx.borrow().0.clone();
            watch_id = input_rx.borrow().1.clone();
        }

        debug!("read_async await ok status: new_line {} seq_id {} watch_id {}", new_line, seq_id, watch_id);

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
                return Ok(None);
            }
        }

        return Err(ReaderError::NoNewLines)
    }

    pub fn interleave_newlines(line: &str, header: Option<Vec<u8>>) -> Vec<u8> {
        let mut acc = vec![];
        let mut v: Vec<u8>;
        let mut citer = line.as_bytes().chunks(USER_LINES);

        while let Some(chunk) = citer.next() {
            // if header specified, prepend to chunked line
            if header.is_some() {
                v = header.as_ref().unwrap().clone();
                let mut c = chunk.to_vec();
                v.append(&mut c);
            } else {
                v = chunk.to_vec();
            }

            v.push(b'\n'); // interleave the chunked line
            acc.push(v);
        }

        acc.into_iter().flatten().collect::<Vec<u8>>()
    }
}
