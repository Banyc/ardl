use std::io;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use num_enum::{IntoPrimitive, TryFromPrimitive};

pub const PUSH_HDR_LEN: usize = 9;
pub const ACK_HDR_LEN: usize = 5;

pub struct StreamFragHeader {
    seq: u32,
    cmd: StreamFragCommand,
}

pub struct StreamFragHeaderBuilder {
    pub seq: u32,
    pub cmd: StreamFragCommand,
}

impl StreamFragHeaderBuilder {
    pub fn build(self) -> Result<StreamFragHeader, Error> {
        let this = StreamFragHeader {
            seq: self.seq,
            cmd: self.cmd,
        };
        this.check_rep();
        Ok(this)
    }
}

pub enum StreamFragCommand {
    Push { len: u32 },
    Ack,
}

#[derive(IntoPrimitive, Clone, Copy, TryFromPrimitive)]
#[repr(u8)]
pub enum CommandType {
    Push,
    Ack,
}

#[derive(Debug)]
pub enum Error {
    Decoding { field: &'static str },
    NotEnoughSpace,
}

impl StreamFragHeader {
    #[inline]
    fn check_rep(&self) {}

    pub fn from_bytes(rdr: &mut io::Cursor<&[u8]>) -> Result<Self, Error> {
        let seq = rdr
            .read_u32::<BigEndian>()
            .map_err(|_e| Error::Decoding { field: "seq" })?;
        let cmd = rdr
            .read_u8()
            .map_err(|_e| Error::Decoding { field: "cmd" })?;
        let cmd = CommandType::try_from(cmd).map_err(|_e| Error::Decoding { field: "cmd" })?;
        let cmd = match cmd {
            CommandType::Push => {
                let len = rdr
                    .read_u32::<BigEndian>()
                    .map_err(|_e| Error::Decoding { field: "len" })?;
                StreamFragCommand::Push { len }
            }
            CommandType::Ack => StreamFragCommand::Ack,
        };

        let this = StreamFragHeader { seq, cmd };
        this.check_rep();
        Ok(this)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut hdr = Vec::new();
        hdr.write_u32::<BigEndian>(self.seq).unwrap();
        let cmd = match self.cmd {
            StreamFragCommand::Push { len: _ } => CommandType::Push.into(),
            StreamFragCommand::Ack => CommandType::Ack.into(),
        };
        hdr.write_u8(cmd).unwrap();
        match self.cmd {
            StreamFragCommand::Push { len } => {
                hdr.write_u32::<BigEndian>(len).unwrap();
            }
            StreamFragCommand::Ack => (),
        }
        hdr
    }

    /// Get a reference to the stream frag header's cmd.
    #[must_use]
    pub fn cmd(&self) -> &StreamFragCommand {
        &self.cmd
    }
}

impl PartialOrd for StreamFragHeader {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.seq.partial_cmp(&other.seq) {
            Some(core::cmp::Ordering::Equal) => {}
            ord => return ord,
        }
        Some(core::cmp::Ordering::Equal)
    }
}

impl PartialEq for StreamFragHeader {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::utils::BufWtr;

    use super::*;

    #[test]
    fn test_1() {
        let hdr = StreamFragHeaderBuilder {
            seq: 345,
            cmd: StreamFragCommand::Push { len: 567 },
        }
        .build()
        .unwrap();
        let mut buf = BufWtr::new(1024, 512);
        buf.prepend(&hdr.to_bytes()).unwrap();
        let mut rdr = Cursor::new(buf.data());
        let hdr2 = StreamFragHeader::from_bytes(&mut rdr).unwrap();
        assert_eq!(hdr.seq, hdr2.seq);
        match hdr.cmd {
            StreamFragCommand::Push { len } => match hdr2.cmd {
                StreamFragCommand::Push { len: len2 } => {
                    assert_eq!(len, len2);
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
}
