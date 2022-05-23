use std::io;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

use crate::utils::{self, BufWtr};

pub const PACKET_HDR_LEN: usize = 6;

pub struct PacketHeader {
    wnd: u16,
    nack: u32,
}

pub struct PacketHeaderBuilder {
    pub wnd: u16,
    pub nack: u32,
}

impl PacketHeaderBuilder {
    pub fn build(self) -> Result<PacketHeader, Error> {
        let this = PacketHeader {
            wnd: self.wnd,
            nack: self.nack,
        };
        this.check_rep();
        Ok(this)
    }
}

#[derive(Debug)]
pub enum Error {
    Decoding { field: &'static str },
    NotEnoughSpace,
}

impl PacketHeader {
    #[inline]
    fn check_rep(&self) {}

    pub fn from_bytes(rdr: &mut io::Cursor<&[u8]>) -> Result<Self, Error> {
        let wnd = rdr
            .read_u16::<BigEndian>()
            .map_err(|_e| Error::Decoding { field: "wnd" })?;
        let nack = rdr
            .read_u32::<BigEndian>()
            .map_err(|_e| Error::Decoding { field: "nack" })?;

        let this = PacketHeader { wnd, nack };
        this.check_rep();
        Ok(this)
    }

    pub fn prepend_to(&self, buf: &mut utils::OwnedBufWtr) -> Result<(), Error> {
        let hdr = self.to_bytes();
        buf.prepend(&hdr).map_err(|_e| Error::NotEnoughSpace)?;
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut hdr = Vec::new();
        hdr.write_u16::<BigEndian>(self.wnd).unwrap();
        hdr.write_u32::<BigEndian>(self.nack).unwrap();
        assert_eq!(hdr.len(), PACKET_HDR_LEN);
        hdr
    }

    #[inline]
    pub fn wnd(&self) -> u16 {
        self.wnd
    }
    
    #[inline]
    pub fn nack(&self) -> u32 {
        self.nack
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::utils::{OwnedBufWtr, BufWtr};

    use super::*;

    #[test]
    fn test_1() {
        let hdr = PacketHeaderBuilder {
            wnd: 123,
            nack: 456,
        }
        .build()
        .unwrap();
        let mut buf = OwnedBufWtr::new(1024, 512);
        buf.prepend(&hdr.to_bytes()).unwrap();
        let mut rdr = Cursor::new(buf.data());
        let hdr2 = PacketHeader::from_bytes(&mut rdr).unwrap();
        assert_eq!(hdr.wnd, hdr2.wnd);
        assert_eq!(hdr.nack, hdr2.nack);
    }
}
