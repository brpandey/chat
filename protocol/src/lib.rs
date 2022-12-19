use tokio_util::codec::{Encoder, Decoder};
use bytes::{Buf, BufMut, BytesMut};
//use tracing::info;

#[derive(Debug)]
pub enum ChatMsg {
    Client(Request),
    Server(Response),
}

#[derive(Debug, Clone)]
pub enum Request { // b'+'
    Users, // b':'
    Quit, //  b'$'
    Message(Vec<u8>), // b'#'
    JoinName(Vec<u8>), // b'&'
/*    Fork { // b'%'
        id: u16,
        name: String,
    },
    Heartbeat,
     */
}

#[derive(Debug, Clone)]
pub enum Response { // b'-'
    UserMessage { // b'*'
        id: u16,
        msg: Vec<u8>, // support a single line for now -- not multi-line
    },
    Notification( // b'@', use for join and leave events, user lists etc..
        Vec<u8>
    ),
    /*
    ForkOk {
        peer_id: u16,
        peer_name: String,
        peer_addr: SocketAddr,
    },
    ForkPeerDown,
    Heartbeat,
    */
}



pub struct ChatCodec; // unit struct

// convert bytes to ProtocolMsgType enum
impl Decoder for ChatCodec {
    type Item = ChatMsg;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let id: u16;

        if src.is_empty() || src.len() < 2 {
            return Ok(None)
        }

        if src[0] == b'+' {
            src.advance(1);
            match src.get_u8() {
                b':' => return Ok(Some(ChatMsg::Client(Request::Users))),
                b'$' => return Ok(Some(ChatMsg::Client(Request::Quit))),
                b'#' => {
                    let msg = decode_vec(src)?;
                    return Ok(Some(ChatMsg::Client(Request::Message(msg))))
                },
                b'&' => {
                    let msg = decode_vec(src)?;
                    return Ok(Some(ChatMsg::Client(Request::JoinName(msg))))
                },
                _ => unimplemented!()
            }
        } else if src[0] == b'-' {
            src.advance(1);
            match src.get_u8() {
                b'*' => { // todo need to handle case where we don't have enough bytes read..
                    id = src.get_u16(); // id field
                    let msg = decode_vec(src)?;
                    return Ok(Some(ChatMsg::Server(Response::UserMessage{id, msg})))
                },
                b'@' => {
                    let msg = decode_vec(src)?;
                    return Ok(Some(ChatMsg::Server(Response::Notification(msg))))
                },
                _ => unimplemented!()
            }
        } else {
            unimplemented!()
        }
    }
}

// Take type T from Encoder<T> and convert it to bytes
impl Encoder<Request> for ChatCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Request, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            Request::Users => {
                dst.put_u8(b'+');
                dst.put_u8(b':');
            },
            Request::Quit => {
                dst.put_u8(b'+');
                dst.put_u8(b'$');
            },
            Request::Message(msg) => {
                dst.put_u8(b'+');
                dst.put_u8(b'#');
                encode_vec(msg, dst);
            },
            Request::JoinName(msg) => {
                dst.put_u8(b'+');
                dst.put_u8(b'&');
                encode_vec(msg, dst);
            }
        }
        Ok(())
    }
}

// Take type T from Encoder<T> and convert it to bytes
impl Encoder<Response> for ChatCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Response, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            Response::UserMessage{id, msg} => {
                dst.put_u8(b'-');
                dst.put_u8(b'*');
                dst.put_u16(id);
                encode_vec(msg, dst);
            },
            Response::Notification(msg) => {
                dst.put_u8(b'-');
                dst.put_u8(b'@');
                encode_vec(msg, dst);
            }
        }

        Ok(())
    }
}

// read bytes from BytesMut into String value 
fn decode_vec(src: &mut BytesMut) -> Result<Vec<u8>, std::io::Error> {
    let str_len = src.get_u16(); // str_len field
    let mut bytes: Vec<u8> = vec![0; str_len as usize]; // reserve a Vec
    src.copy_to_slice(&mut bytes);
    Ok(bytes)
}

// write String into bytes in BytesMut
fn encode_vec(msg: Vec<u8>, dst: &mut BytesMut) {
    dst.reserve(2 + msg.len());
    dst.put_u16(msg.len() as u16);
    dst.extend_from_slice(&msg);
}
