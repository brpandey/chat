use std::collections::HashMap;
use std::sync::Arc;
use std::net::SocketAddr;

use tokio::sync::Mutex;
use tokio::net::tcp;
//use tokio_util::codec::FramedWrite;

//use protocol::ChatCodec;

// server type definitions

// current client registry data
pub type Registry = Arc<Mutex<HashMap<u16, RegistryEntry>>>;
pub type RegistryEntry = (SocketAddr, String, tcp::OwnedWriteHalf);
//pub type RegistryEntry = (SocketAddr, String, FramedWrite<tcp::OwnedWriteHalf, ChatCodec>);


#[derive(Debug)]
pub enum MsgType {
    Joined(u16, Vec<u8>),
    JoinedAck(u16, Vec<u8>),
    Message(u16, Vec<u8>),
    MessageSingle(u16, Vec<u8>),
    Exited(Vec<u8>),
    Users(u16),
}
