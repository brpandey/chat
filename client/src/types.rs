
#[derive(Debug)]
pub enum InputMsg {
    NewSession(u16, String),
    UpdatedSessionName(u16, String),
    CloseSession(u16),
    CloseLobby,
}

#[derive(Debug)]
pub enum PeerMsgType {
    Hello(String, Vec<u8>),
    Note(Vec<u8>),
    Leave(Vec<u8>),
}
