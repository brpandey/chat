use tokio_util::codec::{Encoder, Decoder};
use bytes::{Buf, BufMut, BytesMut};

pub enum ChatMsg {
    Request(Request),
    Response(Response),
}

pub enum Request { // b'+'
    Users, // b':'
    Quit, //  b'$'
    Message(String), // b'#'
    JoinName(String), // b'&'
/*    Fork { // b'%'
        id: u16,
        name: String,
    },
    Heartbeat,
     */
}

pub enum Response { // b'-'
    UserMessage { // b'*'
        id: u16,
        msg: String, // support a single line for now -- not multi-line
    },
    Notification( // b'@', use for join and leave events, user lists etc..
        String
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
        loop { // need to remove loop
            let id: u16;

            if src.is_empty() || src.len() < 2 {
                return Ok(None)
            }

            if src[0] == b'+' {
                src.advance(1);
                match src.get_u8() {
                    b':' => return Ok(Some(ChatMsg::Request(Request::Users))),
                    b'$' => return Ok(Some(ChatMsg::Request(Request::Quit))),
                    b'#' => {
                        src.advance(1);
                        let msg = decode_string(src)?;
                        return Ok(Some(ChatMsg::Request(Request::Message(msg))))
                    },
                    b'&' => {
                        src.advance(1);
                        let msg = decode_string(src)?;
                        return Ok(Some(ChatMsg::Request(Request::JoinName(msg))))
                    },
                    _ => unimplemented!()
                }
            } else if src[0] == b'-' {
                match src[1] {
                    b'*' => { // todo need to handle case where we don't have enough bytes read..
                        src.advance(2);
                        id = src.get_u16(); // id field
                        let msg = decode_string(src)?;
                        return Ok(Some(ChatMsg::Response(Response::UserMessage{id, msg})))
                    },
                    b'&' => {
                        src.advance(2);
                        let msg = decode_string(src)?;
                        return Ok(Some(ChatMsg::Response(Response::Notification(msg))))
                    },
                    _ => unimplemented!()
                }
            } else {
                unimplemented!()
            }
        }
    }
}

// Take type T from Encoder<T> and convert it to bytes
impl Encoder<Request> for ChatCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Request, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            Request::Users => {
                dst.put_u8(b':');
            },
            Request::Quit => {
                dst.put_u8(b'$');
            },
            Request::Message(msg) => {
                encode_string(msg, dst);
            },
            Request::JoinName(msg) => {
                encode_string(msg, dst);
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
                dst.put_u16(id);
                encode_string(msg, dst);
            },
            Response::Notification(msg) => {
                encode_string(msg, dst);
            }
        }

        Ok(())
    }
}

// read bytes from BytesMut into String value 
fn decode_string(src: &mut BytesMut) -> Result<String, std::io::Error> {
    let str_len: u16 = src.get_u16(); // str_len field
    let mut bytes: Vec<u8> = vec![0; str_len as usize]; // reserve a Vec
    //    src.read_exact(&mut bytes); // read bytes len into bytes vec
    src.copy_to_slice(&mut bytes);
    String::from_utf8(bytes).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid utf8"))
}

// write String into bytes in BytesMut
fn encode_string(msg: String, dst: &mut BytesMut) {
    dst.reserve(2 + msg.len());
    let len_slice = u16::to_be_bytes(msg.len() as u16);
    dst.extend_from_slice(&len_slice);
    dst.extend_from_slice(msg.as_bytes());
}
