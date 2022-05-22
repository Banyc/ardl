use std::{
    collections::{BTreeMap, LinkedList},
    time,
};

use crate::{
    protocols::stream_frag_hdr::{
        StreamFragCommand, StreamFragHeader, StreamFragHeaderBuilder, ACK_HDR_LEN, PUSH_HDR_LEN,
    },
    utils::{self, BufAny, BufRdr, BufWtr},
};

const MTU: usize = 512;

pub struct YatcpUpload {
    stream_id: u16,
    to_send_queue: LinkedList<utils::BufAny>,
    sending_queue: BTreeMap<u32, SendingFrag>,
    to_ack_queue: LinkedList<u32>,
    receiving_queue_free_len: usize,
    next_seq_to_receive: u32,
    next_seq_to_send: u32,
    re_tx_timeout: time::Duration,
    remote_rwnd: u16,
}

pub struct YatcpUploadBuilder {
    pub stream_id: u16,
    pub receiving_queue_len: usize,
    pub re_tx_timeout: time::Duration,
}

impl YatcpUploadBuilder {
    pub fn build(self) -> YatcpUpload {
        let this = YatcpUpload {
            stream_id: self.stream_id,
            to_send_queue: LinkedList::new(),
            sending_queue: BTreeMap::new(),
            to_ack_queue: LinkedList::new(),
            receiving_queue_free_len: self.receiving_queue_len,
            next_seq_to_receive: 0,
            re_tx_timeout: self.re_tx_timeout,
            next_seq_to_send: 0,
            remote_rwnd: 0,
        };
        this.check_rep();
        this
    }
}

struct SendingFrag {
    seq: u32,
    body: BufWtr,
    last_seen: time::Instant,
}

impl SendingFrag {
    pub fn is_timeout(&self, timeout: &time::Duration) -> bool {
        *timeout <= self.last_seen.elapsed()
    }
}

impl YatcpUpload {
    #[inline]
    fn check_rep(&self) {
        if !self.to_send_queue.is_empty() {
            if let BufAny::Reader(rdr) = self.to_send_queue.front().unwrap() {
                assert!(!rdr.is_empty());
            }
            if let BufAny::Writer(wtr) = self.to_send_queue.front().unwrap() {
                // assert!(wtr.data_len() + PUSH_HDR_LEN <= MTU && PUSH_HDR_LEN <= wtr.front_len());
                assert!(
                    // the existing data should not bigger than the max body length
                    wtr.data_len() + PUSH_HDR_LEN <= MTU
                        // and the trailing buffer should be enough for new data
                        && MTU <= wtr.data_len() + wtr.back_len()
                );
            }
        }
        assert!(PUSH_HDR_LEN < MTU);
        assert!(ACK_HDR_LEN < MTU);
    }

    pub fn to_send(&mut self, buf: utils::BufWtr) {
        if buf.is_empty() {
            return;
        }
        // the existing data should not bigger than the max body length
        if buf.data_len() + PUSH_HDR_LEN <= MTU
            // and the trailing buffer should be enough for new data
            && MTU <= PUSH_HDR_LEN + buf.data_len() + buf.back_len()
        {
            self.to_send_queue.push_back(BufAny::Writer(buf));
        } else {
            let rdr = utils::BufRdr::new(buf);
            self.to_send_queue.push_back(BufAny::Reader(rdr));
        }
        self.check_rep();
    }

    pub fn flush_mtu(&mut self) -> Option<utils::BufWtr> {
        if self.to_ack_queue.is_empty() && self.to_send_queue.is_empty() {
            return None;
        }

        let mut wtr = BufWtr::new(MTU, 0);

        // piggyback ack
        loop {
            if !(wtr.data_len() + ACK_HDR_LEN <= MTU) {
                self.check_rep();
                return Some(wtr);
            }
            let ack = match self.to_ack_queue.pop_front() {
                Some(ack) => ack,
                None => break,
            };
            let hdr = StreamFragHeaderBuilder {
                stream_id: self.stream_id,
                wnd: self.receiving_queue_free_len as u16,
                seq: ack,
                nack: self.next_seq_to_receive,
                cmd: StreamFragCommand::Ack,
            }
            .build()
            .unwrap();
            wtr.append(&hdr.to_bytes()).unwrap();
        }

        // write push from sending
        for (_seq, frag) in self.sending_queue.iter() {
            if !(wtr.data_len() + PUSH_HDR_LEN <= MTU) {
                self.check_rep();
                return Some(wtr);
            }
            if !frag.is_timeout(&self.re_tx_timeout) {
                continue;
            }
            if !(frag.body.data().len() + PUSH_HDR_LEN <= MTU) {
                continue;
            }
            let hdr = self.build_push_header(frag.seq, frag.body.data().len() as u32);
            wtr.append(&hdr.to_bytes()).unwrap();
            wtr.append(frag.body.data()).unwrap();
        }

        if !(wtr.data_len() + PUSH_HDR_LEN <= MTU) {
            self.check_rep();
            return Some(wtr);
        }
        if !(self.sending_queue.len() < u16::max(self.remote_rwnd, 1) as usize) {
            self.check_rep();
            if wtr.is_empty() {
                return None;
            } else {
                return Some(wtr);
            }
        }

        // move data from to_send queue to sending queue and output those data
        {
            // get as many bytes from to_send_queue
            // call those bytes "frag"
            let frag_body_limit = MTU - PUSH_HDR_LEN - wtr.data_len();
            let mut frag_body = BufWtr::new(frag_body_limit, 0);
            while !self.to_send_queue.is_empty() {
                match self.to_send_queue.pop_front().unwrap() {
                    BufAny::Reader(mut rdr) => {
                        let buf = rdr
                            .try_read(frag_body_limit - frag_body.data_len())
                            .unwrap();
                        frag_body.append(&buf.data()).unwrap();
                        if !rdr.is_empty() {
                            self.to_send_queue.push_front(BufAny::Reader(rdr));
                        }
                        if frag_body.data_len() == frag_body_limit {
                            break;
                        }
                        assert!(frag_body.data_len() < frag_body_limit);
                    }
                    BufAny::Writer(x) => {
                        if frag_body.is_empty() {
                            frag_body.assign(x);
                        } else {
                            let rdr = BufRdr::new(x);
                            self.to_send_queue.push_front(BufAny::Reader(rdr));
                        }
                    }
                }
            }
            assert!(frag_body.data_len() <= frag_body_limit);

            let seq = self.next_seq_to_send;
            self.next_seq_to_send += 1;
            let frag = SendingFrag {
                seq,
                body: frag_body,
                last_seen: time::Instant::now(),
            };
            // write the frag to output buffer
            let hdr = self.build_push_header(frag.seq, frag.body.data().len() as u32);
            wtr.append(&hdr.to_bytes()).unwrap();
            wtr.append(frag.body.data()).unwrap();
            // register the frag to sending_queue
            self.sending_queue.insert(frag.seq, frag);
        }

        self.check_rep();
        assert!(!wtr.is_empty());
        return Some(wtr);
    }

    fn build_push_header(&self, seq: u32, len: u32) -> StreamFragHeader {
        let hdr = StreamFragHeaderBuilder {
            stream_id: self.stream_id,
            wnd: self.receiving_queue_free_len as u16,
            seq,
            nack: self.next_seq_to_receive,
            cmd: StreamFragCommand::Push { len },
        }
        .build()
        .unwrap();
        hdr
    }

    /// Set the yatcp upload's remote rwnd.
    #[inline]
    pub fn set_remote_rwnd(&mut self, remote_rwnd: u16) {
        self.remote_rwnd = remote_rwnd;
        self.check_rep();
    }

    /// Set the yatcp upload's next seq to receive.
    #[inline]
    pub fn set_next_seq_to_receive(&mut self, next_seq_to_receive: u32) {
        self.next_seq_to_receive = next_seq_to_receive;
        self.check_rep();
    }

    #[inline]
    pub fn add_to_ack(&mut self, to_ack: u32) {
        self.to_ack_queue.push_back(to_ack);
        self.check_rep();
    }

    /// Set the yatcp upload's receiving queue free len.
    pub fn set_receiving_queue_free_len(&mut self, receiving_queue_free_len: usize) {
        self.receiving_queue_free_len = receiving_queue_free_len;
    }
}

#[cfg(test)]
mod tests {
    use std::time;

    use crate::{
        protocols::stream_frag_hdr::PUSH_HDR_LEN, utils::BufWtr,
        yatcp::yatcp_upload::YatcpUploadBuilder,
    };

    use super::MTU;

    #[test]
    fn test_empty() {
        let mut upload = YatcpUploadBuilder {
            stream_id: 1,
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let buf = BufWtr::new(MTU / 2, 0);
        upload.to_send(buf);
        let output = upload.flush_mtu();
        assert!(output.is_none());
    }

    #[test]
    fn test_few_1() {
        let mut upload = YatcpUploadBuilder {
            stream_id: 1,
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = BufWtr::new(MTU / 2, 0);
        let origin = vec![0, 1, 2];
        buf.append(&origin).unwrap();
        upload.to_send(buf);
        let output = upload.flush_mtu();
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), PUSH_HDR_LEN + origin.len());
                assert_eq!(x.data()[PUSH_HDR_LEN..], origin);
            }
            None => panic!(),
        }
        assert!(upload.flush_mtu().is_none());
    }

    #[test]
    fn test_few_2() {
        let mut upload = YatcpUploadBuilder {
            stream_id: 1,
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = BufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        upload.to_send(buf);
        let mut buf = BufWtr::new(MTU / 2, 0);
        let origin2 = vec![3, 4];
        buf.append(&origin2).unwrap();
        upload.to_send(buf);
        let output = upload.flush_mtu();
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), PUSH_HDR_LEN + origin1.len() + origin2.len());
                assert_eq!(
                    x.data()[PUSH_HDR_LEN..PUSH_HDR_LEN + origin1.len()],
                    origin1
                );
                assert_eq!(x.data()[PUSH_HDR_LEN + origin1.len()..], origin2);
            }
            None => panic!(),
        }
        assert!(upload.flush_mtu().is_none());
    }

    #[test]
    fn test_few_many() {
        let mut upload = YatcpUploadBuilder {
            stream_id: 1,
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = BufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        upload.to_send(buf);
        let mut buf = BufWtr::new(MTU, 0);
        let origin2 = vec![3; MTU];
        buf.append(&origin2).unwrap();
        upload.to_send(buf);
        let output = upload.flush_mtu();
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), MTU);
                assert_eq!(
                    x.data()[PUSH_HDR_LEN..PUSH_HDR_LEN + origin1.len()],
                    origin1
                );
                assert_eq!(
                    x.data()[PUSH_HDR_LEN + origin1.len()..],
                    origin2[..MTU - PUSH_HDR_LEN - origin1.len()]
                );
            }
            None => panic!(),
        }
        let output = upload.flush_mtu();
        assert!(output.is_none());

        upload.set_remote_rwnd(10);

        let output = upload.flush_mtu();
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), PUSH_HDR_LEN + PUSH_HDR_LEN + origin1.len());
                assert_eq!(
                    x.data()[PUSH_HDR_LEN..],
                    origin2[MTU - PUSH_HDR_LEN - origin1.len()..]
                );
            }
            None => panic!(),
        }
        assert!(upload.flush_mtu().is_none());
    }

    #[test]
    fn test_many_few() {
        let mut upload = YatcpUploadBuilder {
            stream_id: 1,
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = BufWtr::new(MTU, 0);
        let origin1 = vec![3; MTU];
        buf.append(&origin1).unwrap();
        upload.to_send(buf);
        let mut buf = BufWtr::new(MTU / 2, 0);
        let origin2 = vec![0, 1, 2];
        buf.append(&origin2).unwrap();
        upload.to_send(buf);
        let output = upload.flush_mtu();
        // output: hdr mtu-hdr
        // origin:     1[0..mtu-hdr]
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), MTU);
                assert_eq!(x.data()[PUSH_HDR_LEN..], origin1[..MTU - PUSH_HDR_LEN],);
            }
            None => panic!(),
        }
        let output = upload.flush_mtu();
        assert!(output.is_none());

        upload.set_remote_rwnd(10);

        let output = upload.flush_mtu();
        // output: hdr hdr             3
        // origin:     1[mtu-hdr..mtu] 2[0..3]
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), PUSH_HDR_LEN + PUSH_HDR_LEN + origin2.len());
                assert_eq!(
                    x.data()[PUSH_HDR_LEN..PUSH_HDR_LEN + PUSH_HDR_LEN],
                    origin1[MTU - PUSH_HDR_LEN..]
                );
                assert_eq!(
                    x.data()
                        [PUSH_HDR_LEN + PUSH_HDR_LEN..PUSH_HDR_LEN + PUSH_HDR_LEN + origin2.len()],
                    origin2
                );
            }
            None => panic!(),
        }
        assert!(upload.flush_mtu().is_none());
    }
}
