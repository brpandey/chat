use std::{thread, time::Duration};
use tokio::io::{self}; //, AsyncWriteExt}; //, AsyncReadExt};

use client::client::Client;
use client::peer_set::PeerSet;
use client::input_handler::InputHandler;

use tracing_subscriber::fmt;
use tracing::{Level /*, debug*/};

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact() // use abbreviated log format
        .with_max_level(Level::INFO)
        .with_thread_ids(true) // display thread id where event happens
        .with_line_number(true)
        .init(); // set as default subscriber

    let input = InputHandler::new();

    // Notice that client and peer set are different abstractions and can occur
    // independent of each other
    let mut pset = PeerSet::new();
    let client_handle = Client::spawn(input.get_shared(), pset.clone());

    thread::sleep(Duration::from_millis(4000));

    let (input_thread_handle, input_task_handle) = InputHandler::spawn(input);

    // if client has finished, wait on peer clients
    // if peer clients are finished, and peer set is empty kill input handler
    client_handle.await.unwrap();

    // Wait until peer set tasks have finished if any are outstanding
    pset.join_all().await.expect("Unable to join on peer clients set");

    if pset.is_empty().await { // if no peer clients running, kill input handler and terminate
        input_task_handle.abort();
        drop(input_thread_handle);
    }

    println!("Main shutting down");

    Ok(())
}

