
#[derive(Debug)]
pub enum InputMsg {
    NewSession(u16, String),
    UpdatedSessionName(u16, String),
    CloseSession(u16),
    CloseLobby,
}

#[derive(Debug)]
pub enum PeerMsg {
    Hello(String, Vec<u8>),
    Note(Vec<u8>),
    Leave(Vec<u8>),
}

// Custom error implementations, leveraging thiserror
// to handle Debug/Display traits required by the Error trait

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    ChatNameInputError(#[from] std::io::Error),
    #[error("Did not receive Join Name acknowledgement from Main Server")]
    MissingServerJoinHandshake,
}

#[derive(Debug, thiserror::Error)]
pub enum ReaderError {
    #[error("No new lines as cmd line input sender has dropped")]
    NoNewLines, // &'static str
}


#[derive(Debug, thiserror::Error)]
pub enum PeerSetError {
    #[error("arc joinset has other active references thus unable to unwrap")]
    ArcUnwrapError,
}
