use std::{thread, time::Duration};
use tokio::io::{self}; //, AsyncWriteExt}; //, AsyncReadExt};

use client::client::Client;
use client::peer_set::{PeerSet, PeerNames};
use client::input_handler::InputHandler;
use client::event_bus::EventBus;

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

    let input = InputHandler::new(); // spawn thread to handle reading from stdin and tokio task
    let eb = EventBus::new(); // handle coordination between application, and <--> input handler, peer set
    let eb2 = eb.clone();

    let peer_set = PeerSet::new(); // spawn and track peer client tasks
    let names = PeerNames::new(); // track peer names

    let mut pset = peer_set.clone();

    // Note, client and peer set are different abstractions and
    // hence can occur independent of each other

    let eb_handle = EventBus::spawn(eb, input.get_notifier(), input.get_shared(), peer_set, names.clone());

    let client_handle = Client::spawn(input.get_shared(), names, eb2);

    thread::sleep(Duration::from_millis(4000));

    let (input_thread_handle, input_task_handle) = InputHandler::spawn(input);

    // if client has finished, wait on peer clients
    // if peer clients are finished, and peer set is empty kill input handler
    client_handle.await.unwrap();

    thread::sleep(Duration::from_millis(1000));

    // Wait until peer set tasks have finished if any are outstanding
    pset.join_all().await.expect("Unable to join on peer clients set");

    if pset.is_empty().await { // if no peer clients running, kill input handler and event bus and terminate
        eb_handle.abort();
        input_task_handle.abort();
        drop(input_thread_handle);
    }

    println!("Terminating...");

    Ok(())
}

