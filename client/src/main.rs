use tokio::io::{self}; //, AsyncWriteExt}; //, AsyncReadExt};

use tracing_subscriber::fmt;
use tracing::{Level, /*info*/};

use client::client::Client;

#[tokio::main]
async fn main() -> io::Result<()> {
    fmt()
        .compact() // use abbreviated log format
        .with_max_level(Level::INFO)
        .with_thread_ids(true) // display thread id where event happens
        .init(); // set as default subscriber

    let handle = Client::spawn();

    handle.await.unwrap();

    Ok(())
}

