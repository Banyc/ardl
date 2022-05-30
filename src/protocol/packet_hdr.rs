use std::io::Cursor;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

use crate::utils::{
    buf::{BufSlice, BufWtr},
    Seq32,
};

use super::{DecodingError, EncodingError};

pub const PACKET_HDR_LEN: usize = 6;

pub struct PacketHeader {
    rwnd: u16,
    nack: Seq32,
}

pub struct PacketHeaderBuilder {
    pub rwnd: u16,
    pub nack: Seq32,
}

impl PacketHeaderBuilder {
    pub fn build(self) -> Result<PacketHeader, Error> {
        let this = PacketHeader {
            rwnd: self.rwnd,
            nack: self.nack,
        };
        this.check_rep();
        Ok(this)
    }
}

#[derive(Debug)]
pub enum Error {}

impl PacketHeader {
    #[inline]
    fn check_rep(&self) {}

    #[must_use]
    pub fn from_slice(slice: &mut BufSlice) -> Result<Self, DecodingError> {
        let mut rdr = Cursor::new(slice.data());
        let rwnd = rdr
            .read_u16::<BigEndian>()
            .map_err(|_e| DecodingError::Decoding { field: "rwnd" })?;
        let nack = rdr
            .read_u32::<BigEndian>()
            .map_err(|_e| DecodingError::Decoding { field: "nack" })?;
        let nack = Seq32::from_u32(nack);

        let rdr_len = rdr.position() as usize;
        slice.pop_front(rdr_len).unwrap();

        let this = PacketHeader { rwnd, nack };
        this.check_rep();
        Ok(this)
    }

    #[must_use]
    pub fn append_to(&self, wtr: &mut impl BufWtr) -> Result<(), EncodingError> {
        let mut hdr = Vec::new();
        hdr.write_u16::<BigEndian>(self.rwnd).unwrap();
        hdr.write_u32::<BigEndian>(self.nack.to_u32()).unwrap();
        assert_eq!(hdr.len(), PACKET_HDR_LEN);

        wtr.append(&hdr)
            .map_err(|_| EncodingError::NotEnoughSpace)?;
        Ok(())
    }

    #[must_use]
    #[inline]
    pub fn rwnd(&self) -> u16 {
        self.rwnd
    }

    #[must_use]
    #[inline]
    pub fn nack(&self) -> Seq32 {
        self.nack
    }
}

#[cfg(test)]
mod tests {

    use crate::utils::buf::OwnedBufWtr;

    use super::*;

    #[test]
    fn test1() {
        let hdr1 = PacketHeaderBuilder {
            rwnd: 123,
            nack: Seq32::from_u32(456),
        }
        .build()
        .unwrap();
        let mut wtr = OwnedBufWtr::new(1024, 512);
        hdr1.append_to(&mut wtr).unwrap();
        let hdr2 = PacketHeader::from_slice(&mut BufSlice::from_wtr(wtr)).unwrap();
        assert_eq!(hdr1.rwnd, hdr2.rwnd);
        assert_eq!(hdr1.nack, hdr2.nack);
    }
}
