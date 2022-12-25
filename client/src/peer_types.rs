
pub enum PeerMsgType {
    Hello(u16, Vec<u8>),
    Note(u16, Vec<u8>),
    Leave(u16),
}
