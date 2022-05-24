use std::io;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use num_enum::{IntoPrimitive, TryFromPrimitive};

pub const PUSH_HDR_LEN: usize = 9;
pub const ACK_HDR_LEN: usize = 5;

pub struct FragHeader {
    seq: u32,
    cmd: FragCommand,
}

pub struct FragHeaderBuilder {
    pub seq: u32,
    pub cmd: FragCommand,
}

impl FragHeaderBuilder {
    pub fn build(self) -> Result<FragHeader, Error> {
        let this = FragHeader {
            seq: self.seq,
            cmd: self.cmd,
        };
        this.check_rep();
        Ok(this)
    }
}

pub enum FragCommand {
    Push { len: u32 },
    Ack,
}

#[derive(IntoPrimitive, TryFromPrimitive)]
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

impl FragHeader {
    #[inline]
    fn check_rep(&self) {
        if let FragCommand::Push { len } = self.cmd {
            assert!(len > 0);
        }
    }

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
                if len == 0 {
                    return Err(Error::Decoding { field: "len" });
                }
                FragCommand::Push { len }
            }
            CommandType::Ack => FragCommand::Ack,
        };

        let this = FragHeader { seq, cmd };
        this.check_rep();
        Ok(this)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut hdr = Vec::new();
        hdr.write_u32::<BigEndian>(self.seq).unwrap();
        let cmd = match self.cmd {
            FragCommand::Push { len: _ } => CommandType::Push,
            FragCommand::Ack => CommandType::Ack,
        };
        hdr.write_u8(cmd.into()).unwrap();
        match self.cmd {
            FragCommand::Push { len } => {
                hdr.write_u32::<BigEndian>(len).unwrap();
                assert_eq!(hdr.len(), PUSH_HDR_LEN);
            }
            FragCommand::Ack => {
                assert_eq!(hdr.len(), ACK_HDR_LEN);
            }
        }
        hdr
    }

    #[must_use]
    #[inline]
    pub fn cmd(&self) -> &FragCommand {
        &self.cmd
    }

    #[must_use]
    #[inline]
    pub fn seq(&self) -> u32 {
        self.seq
    }
}

impl PartialOrd for FragHeader {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.seq.partial_cmp(&other.seq) {
            Some(core::cmp::Ordering::Equal) => {}
            ord => return ord,
        }
        Some(core::cmp::Ordering::Equal)
    }
}

impl PartialEq for FragHeader {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::utils::{BufWtr, OwnedBufWtr};

    use super::*;

    #[test]
    fn test_1() {
        let hdr = FragHeaderBuilder {
            seq: 345,
            cmd: FragCommand::Push { len: 567 },
        }
        .build()
        .unwrap();
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.prepend(&hdr.to_bytes()).unwrap();
        let mut rdr = Cursor::new(buf.data());
        let hdr2 = FragHeader::from_bytes(&mut rdr).unwrap();
        assert_eq!(hdr.seq, hdr2.seq);
        match hdr.cmd {
            FragCommand::Push { len } => match hdr2.cmd {
                FragCommand::Push { len: len2 } => {
                    assert_eq!(len, len2);
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
}
