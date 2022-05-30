use std::{io::Cursor, sync::Arc};

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::utils::{
    buf::{BufPasta, BufSlice, BufWtr},
    Seq,
};

use super::{DecodingError, EncodingError};

pub const PUSH_HDR_LEN: usize = 9;
pub const ACK_HDR_LEN: usize = 5;

pub struct Frag {
    seq: Seq,
    cmd: FragCommand,
}

pub struct FragBuilder {
    pub seq: Seq,
    pub cmd: FragCommand,
}

impl FragBuilder {
    pub fn build(self) -> Result<Frag, Error> {
        if let FragCommand::Push { body } = &self.cmd {
            if body.is_empty() {
                return Err(Error::EmptyBody);
            }
        }
        let this = Frag {
            seq: self.seq,
            cmd: self.cmd,
        };
        this.check_rep();
        Ok(this)
    }
}

pub enum FragCommand {
    Push { body: Body },
    Ack,
}

pub enum Body {
    Slice(BufSlice),
    Pasta(Arc<BufPasta>),
}

impl Body {
    pub fn is_empty(&self) -> bool {
        match self {
            Body::Slice(x) => x.is_empty(),
            Body::Pasta(x) => x.is_empty(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Body::Slice(x) => x.len(),
            Body::Pasta(x) => x.len(),
        }
    }
}

impl Frag {
    fn check_rep(&self) {
        if let FragCommand::Push { body } = &self.cmd {
            assert!(!body.is_empty());
        }
    }

    pub fn from_slice(slice: &mut BufSlice) -> Result<Self, DecodingError> {
        let mut rdr = Cursor::new(slice.data());
        let seq = rdr
            .read_u32::<BigEndian>()
            .map_err(|_e| DecodingError::Decoding { field: "seq" })?;
        let seq = Seq::from_u32(seq);
        let cmd = rdr
            .read_u8()
            .map_err(|_e| DecodingError::Decoding { field: "cmd" })?;
        let cmd =
            CommandType::try_from(cmd).map_err(|_e| DecodingError::Decoding { field: "cmd" })?;
        let cmd = match cmd {
            CommandType::Push => {
                let len = rdr
                    .read_u32::<BigEndian>()
                    .map_err(|_e| DecodingError::Decoding { field: "len" })?
                    as usize;
                if len == 0 {
                    return Err(DecodingError::Decoding { field: "len" });
                }
                let rdr_len = rdr.position() as usize;
                drop(rdr);
                slice.pop_front(rdr_len).unwrap();
                let body = slice
                    .pop_front(len)
                    .map_err(|_e| DecodingError::Decoding { field: "body" })?;
                let body = Body::Slice(body);
                FragCommand::Push { body }
            }
            CommandType::Ack => {
                let rdr_len = rdr.position() as usize;
                slice.pop_front(rdr_len).unwrap();
                FragCommand::Ack
            }
        };

        let this = Frag { seq, cmd };
        this.check_rep();
        Ok(this)
    }

    pub fn append_to(&self, wtr: &mut impl BufWtr) -> Result<(), EncodingError> {
        let mut hdr = Vec::new();
        hdr.write_u32::<BigEndian>(self.seq.to_u32()).unwrap();
        let cmd = match self.cmd {
            FragCommand::Push { body: _ } => CommandType::Push,
            FragCommand::Ack => CommandType::Ack,
        };
        hdr.write_u8(cmd.into()).unwrap();
        match &self.cmd {
            FragCommand::Push { body } => {
                hdr.write_u32::<BigEndian>(body.len() as u32).unwrap();
                assert_eq!(hdr.len(), PUSH_HDR_LEN);
                match body {
                    Body::Slice(body) => {
                        wtr.append(&hdr)
                            .map_err(|_| EncodingError::NotEnoughSpace)?;
                        wtr.append(body.data())
                            .map_err(|_| EncodingError::NotEnoughSpace)?;
                    }
                    Body::Pasta(body) => {
                        wtr.append(&hdr)
                            .map_err(|_| EncodingError::NotEnoughSpace)?;
                        body.append_to(wtr)
                            .map_err(|_| EncodingError::NotEnoughSpace)?;
                    }
                }
            }
            FragCommand::Ack => {
                assert_eq!(hdr.len(), ACK_HDR_LEN);
                wtr.append(&hdr)
                    .map_err(|_| EncodingError::NotEnoughSpace)?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn into_builder(self) -> FragBuilder {
        FragBuilder {
            seq: self.seq,
            cmd: self.cmd,
        }
    }

    #[must_use]
    #[inline]
    pub fn cmd(&self) -> &FragCommand {
        &self.cmd
    }

    #[must_use]
    #[inline]
    pub fn seq(&self) -> Seq {
        self.seq
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match &self.cmd {
            FragCommand::Push { body } => PUSH_HDR_LEN + body.len(),
            FragCommand::Ack => ACK_HDR_LEN,
        }
    }
}

#[derive(IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum CommandType {
    Push,
    Ack,
}

#[derive(Debug)]
pub enum Error {
    EmptyBody,
}

#[cfg(test)]
mod tests {

    use crate::utils::buf::OwnedBufWtr;

    use super::*;

    #[test]
    fn test_push_slice() {
        let frag1 = FragBuilder {
            seq: Seq::from_u32(345),
            cmd: FragCommand::Push {
                body: Body::Slice(BufSlice::from_bytes(vec![0, 1, 2, 3, 4])),
            },
        }
        .build()
        .unwrap();
        let mut wtr = OwnedBufWtr::new(1024, 512);
        frag1.append_to(&mut wtr).unwrap();
        assert_eq!(frag1.len(), wtr.data_len());
        let frag2 = Frag::from_slice(&mut BufSlice::from_wtr(wtr)).unwrap();
        assert_eq!(frag1.seq, frag2.seq);
        match frag1.cmd {
            FragCommand::Push { body: body1 } => match frag2.cmd {
                FragCommand::Push { body: body2 } => {
                    let body1 = match body1 {
                        Body::Slice(x) => x,
                        Body::Pasta(_) => panic!(),
                    };
                    let body2 = match body2 {
                        Body::Slice(x) => x,
                        Body::Pasta(_) => panic!(),
                    };
                    assert_eq!(body1.data(), body2.data());
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn test_push_pasta() {
        let mut pasta = BufPasta::new();
        pasta.append(BufSlice::from_bytes(vec![0, 1, 2, 3, 4]));
        pasta.append(BufSlice::from_bytes(vec![5, 6]));
        let frag1 = FragBuilder {
            seq: Seq::from_u32(345),
            cmd: FragCommand::Push {
                body: Body::Pasta(Arc::new(pasta)),
            },
        }
        .build()
        .unwrap();
        let mut wtr = OwnedBufWtr::new(1024, 512);
        frag1.append_to(&mut wtr).unwrap();
        assert_eq!(frag1.len(), wtr.data_len());
        let frag2 = Frag::from_slice(&mut BufSlice::from_wtr(wtr)).unwrap();
        assert_eq!(frag1.seq, frag2.seq);
        match frag1.cmd {
            FragCommand::Push { body: body1 } => match frag2.cmd {
                FragCommand::Push { body: body2 } => {
                    let body1 = match body1 {
                        Body::Slice(_) => panic!(),
                        Body::Pasta(x) => {
                            let mut wtr = OwnedBufWtr::new(1024, 0);
                            x.append_to(&mut wtr).unwrap();
                            wtr
                        }
                    };
                    let body2 = match body2 {
                        Body::Slice(x) => x,
                        Body::Pasta(_) => panic!(),
                    };
                    assert_eq!(body1.data(), body2.data());
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn test_ack() {
        let frag1 = FragBuilder {
            seq: Seq::from_u32(345),
            cmd: FragCommand::Ack,
        }
        .build()
        .unwrap();
        let mut wtr = OwnedBufWtr::new(1024, 512);
        frag1.append_to(&mut wtr).unwrap();
        assert_eq!(frag1.len(), wtr.data_len());
        let frag2 = Frag::from_slice(&mut BufSlice::from_wtr(wtr)).unwrap();
        assert_eq!(frag1.seq, frag2.seq);
        match frag1.cmd {
            FragCommand::Ack => match frag2.cmd {
                FragCommand::Ack => (),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
}
