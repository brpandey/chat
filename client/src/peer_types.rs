#[derive(Debug)]
pub enum PeerMsgType {
    Hello(String, Vec<u8>),
    Note(Vec<u8>),
    Leave(Vec<u8>),
}
