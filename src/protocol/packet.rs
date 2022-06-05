use super::{frag::Frag, packet_hdr::PacketHeader, DecodingError, EncodingError};
use crate::utils::buf::{BufSlice, BufWtr};

pub struct Packet {
    hdr: PacketHeader,
    frags: Vec<Frag>,
}

pub struct PacketBuilder {
    pub hdr: PacketHeader,
    pub frags: Vec<Frag>,
}

impl PacketBuilder {
    pub fn build(self) -> Result<Packet, Error> {
        let this = Packet {
            hdr: self.hdr,
            frags: self.frags,
        };
        this.check_rep();
        Ok(this)
    }
}

impl Packet {
    fn check_rep(&self) {}

    pub fn from_slice(slice: &mut BufSlice) -> Result<Self, DecodingError> {
        let hdr = PacketHeader::from_slice(slice)?;
        let mut frags = Vec::new();
        while !slice.is_empty() {
            let frag = Frag::from_slice(slice)?;
            frags.push(frag);
        }

        let this = Packet { hdr, frags };
        this.check_rep();
        Ok(this)
    }

    pub fn append_to(&self, wtr: &mut impl BufWtr) -> Result<(), EncodingError> {
        self.hdr.append_to(wtr)?;
        for frag in &self.frags {
            frag.append_to(wtr)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn into_builder(self) -> PacketBuilder {
        PacketBuilder {
            hdr: self.hdr,
            frags: self.frags,
        }
    }

    #[must_use]
    pub fn hdr(&self) -> &PacketHeader {
        &self.hdr
    }

    #[must_use]
    pub fn frags(&self) -> &Vec<Frag> {
        &self.frags
    }
}

#[derive(Debug)]
pub enum Error {}

#[cfg(test)]
mod tests {

    use crate::{
        protocol::{
            frag::{Body, FragBuilder, FragCommand},
            packet_hdr::PacketHeaderBuilder,
        },
        utils::{
            buf::{BufSlice, OwnedBufWtr},
            Seq32,
        },
    };

    use super::{Packet, PacketBuilder};

    #[test]
    fn test1() {
        let packet1 = PacketBuilder {
            hdr: PacketHeaderBuilder {
                rwnd: 123,
                nack: Seq32::from_u32(456),
            }
            .build()
            .unwrap(),
            frags: vec![
                FragBuilder {
                    seq: Seq32::from_u32(345),
                    cmd: FragCommand::Ack,
                }
                .build()
                .unwrap(),
                FragBuilder {
                    seq: Seq32::from_u32(345),
                    cmd: FragCommand::Push {
                        body: Body::Slice(BufSlice::from_bytes(vec![0, 1, 2, 3, 4])),
                    },
                }
                .build()
                .unwrap(),
            ],
        }
        .build()
        .unwrap();
        let mut wtr = OwnedBufWtr::new(1024, 512);
        packet1.append_to(&mut wtr).unwrap();
        let packet2 = Packet::from_slice(&mut BufSlice::from_wtr(wtr)).unwrap();
        assert_eq!(packet1.hdr.rwnd(), packet2.hdr.rwnd());
        assert_eq!(packet1.hdr.nack(), packet2.hdr.nack());
        assert_eq!(packet1.frags.len(), packet2.frags.len());
        assert_eq!(packet1.frags[0].seq(), packet2.frags[0].seq());
        assert_eq!(packet1.frags[1].seq(), packet2.frags[1].seq());
    }
}
