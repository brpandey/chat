#![allow(dead_code)]  // just temporary

use tokio_util::codec::{Encoder, Decoder};
use bytes::{Buf, BufMut, BytesMut};
use std::net::SocketAddr;
use std::str;

// encode and decode bypasses traditional libraries
// like serde or message pack

//use tracing::info;
const REQ: u8 = b'+';
const REQ_USERS: u8 = b':';
const REQ_QUIT: u8 = b'$';
const REQ_MSG: u8 = b'#';
const REQ_NAME: u8 = b'&';
const REQ_FORKP: u8 = b'%';
const REQ_HBEAT: u8 = b'!';

const RESP: u8 = b'-';
const RESP_USERMSG: u8 = b'*';
const RESP_NOTIF: u8 = b'@';
const RESP_FORKACK: u8 = b'^';
const RESP_PEERUNAV: u8 = b'~';
const RESP_HBEAT: u8 = b'!';

const ASK: u8 = b'{';
const ASK_LEAVE: u8 = b'?';
const ASK_NOTE: u8 = b'>';

const REPLY: u8 = b'}';
const REPLY_NOTE: u8 = b'<';

#[derive(Debug)]
pub enum ChatMsg {
    Client(Request),
    Server(Response),
    PeerA(Ask), // from
    PeerB(Reply), // to
}

#[derive(Debug, Clone)]
pub enum Request { // b'+'
    Users, // b':'
    Quit, //  b'$'
    Message(Vec<u8>), // b'#'
    JoinName(Vec<u8>), // b'&'
    ForkPeer{ // b'%'
//        id: u16,
        name: Vec<u8>,
    },
    Heartbeat, // b'!'
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
    ForkPeerAckA { // b'^'   same type this used for decode
        id: u16,
        name: Vec<u8>,
        addr: SocketAddr,
    },
    ForkPeerAckB { // b'^'   same type this used for encode
        id: u16,
        name: Vec<u8>,
        addr: Vec<u8>,
    },
    PeerUnavailable(Vec<u8>), // b'~'
    Heartbeat, // b'!'
}

#[derive(Debug, Clone)]
pub enum Ask { // b'{'
    Hello(Vec<u8>), // b'|'           // peer a hello request
    Leave(Vec<u8>), // b'?'
    Note(Vec<u8>), // b'>'
}

#[derive(Debug, Clone)]
pub enum Reply { // b'}'
    Hello(Vec<u8>), // b'|'          // peer b hello reply
    Leave(Vec<u8>), // b'?'
    Note(Vec<u8>), // b'<'
}

pub struct ChatCodec; // unit struct

// convert bytes to ProtocolMsgType enum
impl Decoder for ChatCodec {
    type Item = ChatMsg;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let id: u16;
        let msg: Vec<u8>;

        if src.is_empty() || src.len() < 2 {
            return Ok(None)
        }

        match src[0] {
            REQ => {
                src.advance(1);
                match src.get_u8() {
                    REQ_USERS => return Ok(Some(ChatMsg::Client(Request::Users))),
                    REQ_QUIT => return Ok(Some(ChatMsg::Client(Request::Quit))),
                    REQ_MSG => {
                        msg = decode_vec(src)?;
                        return Ok(Some(ChatMsg::Client(Request::Message(msg))))
                    },
                    REQ_NAME => {
                        msg = decode_vec(src)?;
                        return Ok(Some(ChatMsg::Client(Request::JoinName(msg))))
                    },
                    REQ_FORKP => {
//                        id = src.get_u16();
                        msg = decode_vec(src)?;
                        return Ok(Some(ChatMsg::Client(Request::ForkPeer{name: msg})))
                    },
                    _ => unimplemented!()
                }
            },
            RESP => {
                src.advance(1);
                match src.get_u8() {
                    RESP_USERMSG => { // todo need to handle case where we don't have enough bytes read..
                        id = src.get_u16(); // id field
                        msg = decode_vec(src)?;
                        return Ok(Some(ChatMsg::Server(Response::UserMessage{id, msg})))
                    },
                    RESP_NOTIF => {
                        msg = decode_vec(src)?;
                        return Ok(Some(ChatMsg::Server(Response::Notification(msg))))
                    },
                    RESP_FORKACK => {
                        id = src.get_u16();
                        let name = decode_vec(src)?;
                        let addr = decode_addr(src)?;
                        return Ok(Some(ChatMsg::Server(Response::ForkPeerAckA{id, name, addr})))
                    },
                    RESP_PEERUNAV => {
                        let name = decode_vec(src)?;
                        return Ok(Some(ChatMsg::Server(Response::PeerUnavailable(name))));
                    },
                    _ => unimplemented!()
                }
            },
            _ => unimplemented!()
        }
    }
}

// Take type T from Encoder<T> and convert it to bytes
impl Encoder<Request> for ChatCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Request, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            Request::Users => {
                dst.put_u8(REQ);
                dst.put_u8(REQ_USERS);
            },
            Request::Quit => {
                dst.put_u8(REQ);
                dst.put_u8(REQ_QUIT);
            },
            Request::Message(msg) => {
                dst.put_u8(REQ);
                dst.put_u8(REQ_MSG);
                encode_vec(msg, dst);
            },
            Request::JoinName(msg) => {
                dst.put_u8(REQ);
                dst.put_u8(REQ_NAME);
                encode_vec(msg, dst);
            },
            Request::ForkPeer{name} => {
                dst.put_u8(REQ);
                dst.put_u8(REQ_FORKP);
                encode_vec(name, dst);
            },
            _ => unimplemented!()
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
                dst.put_u8(RESP);
                dst.put_u8(RESP_USERMSG);
                dst.put_u16(id);
                encode_vec(msg, dst);
            },
            Response::Notification(msg) => {
                dst.put_u8(RESP);
                dst.put_u8(RESP_NOTIF);
                encode_vec(msg, dst);
            },
            Response::ForkPeerAckB{id, name, addr} => {
                dst.put_u8(RESP);
                dst.put_u8(RESP_FORKACK);
                dst.put_u16(id);
                encode_vec(name, dst);
                encode_vec(addr, dst); // upon encode socket addr is Vec<u8
//                encode_addr(addr, dst);
            },
            Response::PeerUnavailable(name) => {
                dst.put_u8(RESP);
                dst.put_u8(RESP_PEERUNAV);
                encode_vec(name, dst);
            },
            _ => unimplemented!()
        }

        Ok(())
    }
}

impl Encoder<ChatMsg> for ChatCodec {
    type Error = std::io::Error;

    fn encode(&mut self, _item: ChatMsg, _dst: &mut BytesMut) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Encoder<Ask> for ChatCodec {
    type Error = std::io::Error;

    fn encode(&mut self, _item: Ask, _dst: &mut BytesMut) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Encoder<Reply> for ChatCodec {
    type Error = std::io::Error;

    fn encode(&mut self, _item: Reply, _dst: &mut BytesMut) -> Result<(), Self::Error> {
        Ok(())
    }
}




// read bytes from BytesMut into Vec<u8> value
fn decode_vec(src: &mut BytesMut) -> Result<Vec<u8>, std::io::Error> {
    let v_len = src.get_u16(); // v_len field
    let mut bytes: Vec<u8> = vec![0; v_len as usize]; // reserve a Vec
    src.copy_to_slice(&mut bytes);
    Ok(bytes)
}

// write Vec<u8> into BytesMut
fn encode_vec(msg: Vec<u8>, dst: &mut BytesMut) {
    dst.reserve(2 + msg.len());
    dst.put_u16(msg.len() as u16);
    dst.extend_from_slice(&msg);
}

// read bytes from BytesMut into SocketAddr type
pub fn decode_addr(src: &mut BytesMut) -> Result<SocketAddr, std::io::Error> {
    let bytes = decode_vec(src)?;
    let addr_str = str::from_utf8(&bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid utf8"))?;

    addr_str.parse().map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Addr parse error"))
}

// write SocketAddr type into BytesMut
pub fn encode_addr(addr: SocketAddr, dst: &mut BytesMut) {
    let bytes = addr.to_string().into_bytes();
    encode_vec(bytes, dst)
}
