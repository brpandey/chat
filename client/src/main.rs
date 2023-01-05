use std::{thread, time::Duration};

use tokio::io::{self}; //, AsyncWriteExt}; //, AsyncReadExt};

use tracing_subscriber::fmt;
use tracing::{Level, /*info*/};

use client::client::Client;
use client::input_handler::InputHandler;

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact() // use abbreviated log format
        .with_max_level(Level::INFO)
        .with_thread_ids(true) // display thread id where event happens
        .with_line_number(true)
        .init(); // set as default subscriber

    let input = InputHandler::new();
    let io_shared = input.get_shared();

    let ch = Client::spawn(io_shared);

    thread::sleep(Duration::from_millis(4000));

    let ih = InputHandler::spawn(input);

    // try_join?
    ch.await.unwrap();
    ih.await.expect("Couldn't join successfully on input task");

    Ok(())
}

