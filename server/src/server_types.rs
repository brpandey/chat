use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::net::tcp;

// Server type definitions

// Client registry data
pub type Registry = Arc<Mutex<HashMap<u16, RegistryEntry>>>;

//                  Socket repr as String, name, tcp write handle
pub type RegistryEntry = (String, String, tcp::OwnedWriteHalf);

#[derive(Debug)]
pub enum MsgType {
    Joined(u16, Vec<u8>),
    JoinedAck(u16, Vec<u8>),
    Message(u16, Vec<u8>),
    MessageSingle(u16, Vec<u8>),
    Exited(Vec<u8>),
    Users(u16),
    ForkPeerAck(u16, u16, Vec<u8>, Vec<u8>),     // cid, pid, name, socket
    PeerUnavailable(u16, Vec<u8>),
}
