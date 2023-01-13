use std::{thread, time::Duration};
use tokio::io::{self}; //, AsyncWriteExt}; //, AsyncReadExt};

use client::client::Client;
use client::peer_set::PeerSet;
use client::input_handler::InputHandler;

use tracing_subscriber::fmt;
use tracing::{Level, info};

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact() // use abbreviated log format
        .with_max_level(Level::INFO)
        .with_thread_ids(true) // display thread id where event happens
        .with_line_number(true)
        .init(); // set as default subscriber

    let input = InputHandler::new();
    let mut pset = PeerSet::new();

    let client_handle = Client::spawn(input.get_shared(), pset.clone());

    thread::sleep(Duration::from_millis(4000));

    let input_handle = InputHandler::spawn(input);

    // if client has finished, wait on peer clients
    // if peer clients are finished, and peer set is empty kill input handler
    client_handle.await.unwrap();

    info!("client handle has finished, now waiting on peer set -- join all");

    pset.join_all().await.expect("Couldn't join successfully on peer clients set");

    info!("Pset finished joining.  Checking if pset is empty now");

    if pset.is_empty().await { // if no peer clients running, kill input handler and terminate
        info!("pset is empty now so aborting input handle");
        input_handle.abort();
//    } else { // else wait for peer clients to finish
//        ih.await.expect("Couldn't join successfully on input task");        
    }

    info!("Main shutting down");

    Ok(())
}

