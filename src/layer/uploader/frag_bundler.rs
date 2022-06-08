use crate::protocol::frag::Frag;
use std::mem;

pub struct FragBundler {
    each_bundle_space: usize,
    bundles: Vec<Vec<Frag>>,

    loading_bundle: Vec<Frag>,
    loading_len: usize,
}

impl FragBundler {
    pub fn check_rep(&self) {
        for bundle in &self.bundles {
            let mut len = 0;
            for frag in bundle {
                len += frag.len();
            }
            assert!(len <= self.each_bundle_space);
        }
        let mut len = 0;
        for frag in &self.loading_bundle {
            len += frag.len();
        }
        assert_eq!(len, self.loading_len);
        assert!(self.loading_len <= self.each_bundle_space);
    }

    #[must_use]
    pub fn new(each_bundle_space: usize) -> Self {
        let this = FragBundler {
            each_bundle_space,
            bundles: Vec::new(),
            loading_bundle: Vec::new(),
            loading_len: 0,
        };
        this.check_rep();
        this
    }

    pub fn pack(&mut self, frag: Frag) -> Result<(), PackError> {
        if !(frag.len() <= self.each_bundle_space) {
            return Err(PackError::FragTooLarge);
        }

        if !(frag.len() + self.loading_len <= self.each_bundle_space) {
            let loading_bundle = mem::replace(&mut self.loading_bundle, Vec::new());
            self.bundles.push(loading_bundle);
            self.loading_bundle = Vec::new();
            self.loading_len = 0;
        }
        self.loading_len += frag.len();
        self.loading_bundle.push(frag);

        self.check_rep();
        Ok(())
    }

    #[must_use]
    pub fn loading_space(&self) -> usize {
        self.each_bundle_space - self.loading_len
    }

    #[must_use]
    pub fn into_bundles(mut self) -> Vec<Vec<Frag>> {
        if self.loading_len > 0 {
            self.bundles.push(self.loading_bundle);
        }
        self.bundles
    }
}

#[derive(Debug)]
pub enum PackError {
    FragTooLarge,
}

#[cfg(test)]
mod tests {
    use crate::{
        protocol::frag::{Body, FragBuilder, FragCommand, ACK_HDR_LEN, PUSH_HDR_LEN},
        utils::{buf::BufSlice, Seq32},
    };

    use super::FragBundler;

    #[test]
    fn test1() {
        let frag1 = FragBuilder {
            seq: Seq32::from_u32(1),
            cmd: FragCommand::Ack,
        }
        .build()
        .unwrap();
        let frag2 = FragBuilder {
            seq: Seq32::from_u32(1),
            cmd: FragCommand::Push {
                body: Body::Slice(BufSlice::from_bytes(vec![9])),
            },
        }
        .build()
        .unwrap();
        let frag3 = FragBuilder {
            seq: Seq32::from_u32(1),
            cmd: FragCommand::Push {
                body: Body::Slice(BufSlice::from_bytes(vec![9])),
            },
        }
        .build()
        .unwrap();

        let mut bundler = FragBundler::new(ACK_HDR_LEN + PUSH_HDR_LEN + 1);
        bundler.pack(frag1).unwrap();
        assert_eq!(bundler.bundles.len(), 0);
        bundler.pack(frag2).unwrap();
        assert_eq!(bundler.bundles.len(), 0);
        bundler.pack(frag3).unwrap();
        assert_eq!(bundler.bundles.len(), 1);

        let bundles = bundler.into_bundles();
        assert_eq!(bundles.len(), 2);
        assert_eq!(bundles[0].len(), 2);
        assert_eq!(bundles[1].len(), 1);
    }
}
