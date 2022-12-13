use std::collections::HashMap;
use std::sync::Arc;
use std::net::SocketAddr;

use tokio::sync::Mutex;
use tokio::net::tcp;


// server type definitions

// current client registry data
pub type Registry = Arc<Mutex<HashMap<usize, RegistryEntry>>>;
pub type RegistryEntry = (SocketAddr, String, tcp::OwnedWriteHalf);


#[derive(Debug)]
pub enum MsgType {
    Joined(usize, Vec<u8>),
    JoinedAck(usize, Vec<u8>),
    Message(usize, Vec<u8>),
    MessageSingle(usize, Vec<u8>),
    Exited(Vec<u8>),
    Users(usize),
}
