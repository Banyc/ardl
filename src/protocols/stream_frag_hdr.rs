use std::io;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::utils;

pub const PUSH_HDR_LEN: usize = 15;
pub const ACK_HDR_LEN: usize = 11;

/// # Packet Format
///
/// ```text
/// 0       2   3   4   5   6       8 (BYTE)
/// +-------+---+---+---+---+-------+
/// |  wnd  |
/// +-------+---+---+---+---+-------+
/// |     seq       |     nack      |
/// +---+-----------+---------------+
/// |cmd|
/// +---+-----------+
/// |     len       |
/// +---------------+---------------+
/// |                               |
/// |        DATA (optional)        |
/// |                               |
/// +-------------------------------+
/// ```
pub struct StreamFragHeader {
    wnd: u16,
    seq: u32,
    nack: u32,
    cmd: StreamFragCommand,
}

pub struct StreamFragHeaderBuilder {
    pub wnd: u16,
    pub seq: u32,
    pub nack: u32,
    pub cmd: StreamFragCommand,
}

impl StreamFragHeaderBuilder {
    pub fn build(self) -> Result<StreamFragHeader, Error> {
        let this = StreamFragHeader {
            wnd: self.wnd,
            seq: self.seq,
            nack: self.nack,
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
        let wnd = rdr
            .read_u16::<BigEndian>()
            .map_err(|_e| Error::Decoding { field: "wnd" })?;
        let seq = rdr
            .read_u32::<BigEndian>()
            .map_err(|_e| Error::Decoding { field: "seq" })?;
        let nack = rdr
            .read_u32::<BigEndian>()
            .map_err(|_e| Error::Decoding { field: "nack" })?;
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

        let this = StreamFragHeader {
            wnd,
            seq,
            nack,
            cmd,
        };
        this.check_rep();
        Ok(this)
    }

    pub fn prepend_to(&self, buf: &mut utils::BufWtr) -> Result<(), Error> {
        let hdr = self.to_bytes();
        buf.prepend(&hdr).map_err(|_e| Error::NotEnoughSpace)?;
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut hdr = Vec::new();
        hdr.write_u16::<BigEndian>(self.wnd).unwrap();
        hdr.write_u32::<BigEndian>(self.seq).unwrap();
        hdr.write_u32::<BigEndian>(self.nack).unwrap();
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
            wnd: 234,
            cmd: StreamFragCommand::Push { len: 567 },
            seq: 345,
            nack: 456,
        }
        .build()
        .unwrap();
        let mut buf = BufWtr::new(1024, 512);
        hdr.prepend_to(&mut buf).unwrap();
        let mut rdr = Cursor::new(buf.data());
        let hdr2 = StreamFragHeader::from_bytes(&mut rdr).unwrap();
        assert_eq!(hdr.wnd, hdr2.wnd);
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
