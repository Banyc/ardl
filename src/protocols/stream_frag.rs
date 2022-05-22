use crate::utils;

use super::stream_frag_hdr::{StreamFragCommand, StreamFragHeader};

pub struct StreamFrag {
    header: StreamFragHeader,
    body: Option<utils::BufWtr>,
}

pub struct StreamFragBuilder {
    pub header: StreamFragHeader,
    pub body: Option<utils::BufWtr>,
}

impl StreamFragBuilder {
    pub fn build(self) -> StreamFrag {
        let this = StreamFrag {
            header: self.header,
            body: self.body,
        };
        this.check_rep();
        this
    }
}

impl StreamFrag {
    #[inline]
    fn check_rep(&self) {
        match self.header.cmd() {
            StreamFragCommand::Push { len } => {
                assert!(self.body.is_some());
                let data_len = self.body.as_ref().unwrap().data_len();
                assert_eq!(data_len, *len as usize);
            }
            StreamFragCommand::Ack => assert!(self.body.is_none()),
        }
    }
}

impl PartialOrd for StreamFrag {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.header.partial_cmp(&other.header) {
            Some(core::cmp::Ordering::Equal) => {}
            ord => return ord,
        }
        Some(core::cmp::Ordering::Equal)
    }
}

impl PartialEq for StreamFrag {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header
    }
}
