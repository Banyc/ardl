use std::{
    collections::{BTreeMap, LinkedList},
    time,
};

use crate::{
    protocols::yatcp::{
        frag_hdr::{FragCommand, FragHeader, FragHeaderBuilder, ACK_HDR_LEN, PUSH_HDR_LEN},
        packet_hdr::{PacketHeaderBuilder, PACKET_HDR_LEN},
    },
    utils::{self, BufPasta, BufRdr, BufWtr},
};

const MTU: usize = 512;

pub struct YatcpUpload {
    to_send_queue: LinkedList<utils::BufRdr>,
    sending_queue: BTreeMap<u32, SendingFrag>,
    to_ack_queue: LinkedList<u32>,
    receiving_queue_free_len: usize,
    next_seq_to_receive: u32,
    next_seq_to_send: u32,
    re_tx_timeout: time::Duration,
    remote_rwnd: u16,
}

pub struct YatcpUploadBuilder {
    pub receiving_queue_len: usize,
    pub re_tx_timeout: time::Duration,
}

impl YatcpUploadBuilder {
    pub fn build(self) -> YatcpUpload {
        let this = YatcpUpload {
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
    body: BufPasta,
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
            assert!(!self.to_send_queue.front().unwrap().is_empty());
        }
        assert!(PACKET_HDR_LEN + PUSH_HDR_LEN < MTU);
        assert!(PACKET_HDR_LEN + ACK_HDR_LEN < MTU);
    }

    pub fn to_send(&mut self, buf: utils::BufWtr) {
        if buf.is_empty() {
            return;
        }
        let rdr = BufRdr::new(buf);
        self.to_send_queue.push_back(rdr);
        self.check_rep();
    }

    pub fn flush_packet(&mut self) -> Option<utils::BufWtr> {
        match self.flush_frags() {
            Some(mut wtr) => {
                // packet header
                let hdr = PacketHeaderBuilder {
                    wnd: self.receiving_queue_free_len as u16,
                    nack: self.next_seq_to_receive,
                }
                .build()
                .unwrap();
                wtr.prepend(&hdr.to_bytes()).unwrap();
                Some(wtr)
            }
            None => None,
        }
    }

    #[inline]
    fn flush_frags(&mut self) -> Option<utils::BufWtr> {
        if self.to_ack_queue.is_empty() && self.to_send_queue.is_empty() {
            return None;
        }

        let mut wtr = BufWtr::new(MTU, PACKET_HDR_LEN);

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
            let hdr = FragHeaderBuilder {
                seq: ack,
                cmd: FragCommand::Ack,
            }
            .build()
            .unwrap();
            wtr.append(&hdr.to_bytes()).unwrap();
        }

        // write push from sending
        for (_seq, frag) in self.sending_queue.iter() {
            if !(wtr.data_len() + PUSH_HDR_LEN < MTU - PACKET_HDR_LEN) {
                self.check_rep();
                return Some(wtr);
            }
            if !frag.is_timeout(&self.re_tx_timeout) {
                continue;
            }
            if !(frag.body.len() + PUSH_HDR_LEN <= MTU - PACKET_HDR_LEN) {
                continue;
            }
            let hdr = self.build_push_header(frag.seq, frag.body.len() as u32);
            wtr.append(&hdr.to_bytes()).unwrap();
            frag.body.append_to(&mut wtr).unwrap();
        }

        if !(wtr.data_len() + PUSH_HDR_LEN < MTU - PACKET_HDR_LEN) {
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
            let frag_body_limit = MTU - PACKET_HDR_LEN - PUSH_HDR_LEN - wtr.data_len();
            assert!(frag_body_limit != 0);
            let mut frag_body = BufPasta::new();
            while !self.to_send_queue.is_empty() {
                let mut rdr = self.to_send_queue.pop_front().unwrap();
                let buf = rdr.try_read(frag_body_limit - frag_body.len()).unwrap();
                frag_body.append(buf);
                if !rdr.is_empty() {
                    self.to_send_queue.push_front(rdr);
                }
                if frag_body.len() == frag_body_limit {
                    break;
                }
                assert!(frag_body.len() < frag_body_limit);
            }
            assert!(frag_body.len() <= frag_body_limit);

            let seq = self.next_seq_to_send;
            self.next_seq_to_send += 1;
            let frag = SendingFrag {
                seq,
                body: frag_body,
                last_seen: time::Instant::now(),
            };
            // write the frag to output buffer
            let hdr = self.build_push_header(frag.seq, frag.body.len() as u32);
            wtr.append(&hdr.to_bytes()).unwrap();
            frag.body.append_to(&mut wtr).unwrap();
            // register the frag to sending_queue
            self.sending_queue.insert(frag.seq, frag);
        }

        self.check_rep();
        assert!(!wtr.is_empty());
        return Some(wtr);
    }

    fn build_push_header(&self, seq: u32, len: u32) -> FragHeader {
        let hdr = FragHeaderBuilder {
            seq,
            cmd: FragCommand::Push { len },
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
        protocols::yatcp::{frag_hdr::PUSH_HDR_LEN, packet_hdr::PACKET_HDR_LEN},
        utils::BufWtr,
        yatcp::yatcp_upload::YatcpUploadBuilder,
    };

    use super::MTU;

    #[test]
    fn test_empty() {
        let mut upload = YatcpUploadBuilder {
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let buf = BufWtr::new(MTU / 2, 0);
        upload.to_send(buf);
        let output = upload.flush_packet();
        assert!(output.is_none());
    }

    #[test]
    fn test_few_1() {
        let mut upload = YatcpUploadBuilder {
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = BufWtr::new(MTU / 2, 0);
        let origin = vec![0, 1, 2];
        buf.append(&origin).unwrap();
        upload.to_send(buf);
        let output = upload.flush_packet();
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), PACKET_HDR_LEN + PUSH_HDR_LEN + origin.len());
                assert_eq!(x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..], origin);
            }
            None => panic!(),
        }
        assert!(upload.flush_packet().is_none());
    }

    #[test]
    fn test_few_2() {
        let mut upload = YatcpUploadBuilder {
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
        let output = upload.flush_packet();
        match output {
            Some(x) => {
                assert_eq!(
                    x.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len() + origin2.len()
                );
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()],
                    origin1
                );
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()..],
                    origin2
                );
            }
            None => panic!(),
        }
        assert!(upload.flush_packet().is_none());
    }

    #[test]
    fn test_few_many() {
        let mut upload = YatcpUploadBuilder {
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
        let output = upload.flush_packet();
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), MTU);
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()],
                    origin1
                );
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()..],
                    origin2[..MTU - PACKET_HDR_LEN - PUSH_HDR_LEN - origin1.len()]
                );
            }
            None => panic!(),
        }
        let output = upload.flush_packet();
        assert!(output.is_none());

        upload.set_remote_rwnd(10);

        let output = upload.flush_packet();
        match output {
            Some(x) => {
                assert_eq!(
                    x.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()
                );
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..],
                    origin2[MTU - PACKET_HDR_LEN - PUSH_HDR_LEN - origin1.len()..]
                );
            }
            None => panic!(),
        }
        assert!(upload.flush_packet().is_none());
    }

    #[test]
    fn test_many_few() {
        let mut upload = YatcpUploadBuilder {
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
        let output = upload.flush_packet();
        // output: _hdr hdr mtu-_hdr-hdr
        // origin:          1[0..mtu-_hdr-hdr]
        match output {
            Some(x) => {
                assert_eq!(x.data_len(), MTU);
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..],
                    origin1[..MTU - PACKET_HDR_LEN - PUSH_HDR_LEN],
                );
            }
            None => panic!(),
        }
        let output = upload.flush_packet();
        assert!(output.is_none());

        upload.set_remote_rwnd(10);

        let output = upload.flush_packet();
        // output: _hdr hdr _hdr+hdr             3
        // origin:          1[mtu-_hdr-hdr..mtu] 2[0..3]
        match output {
            Some(x) => {
                assert_eq!(
                    x.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN + origin2.len()
                );
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN],
                    origin1[MTU - PACKET_HDR_LEN - PUSH_HDR_LEN..]
                );
                assert_eq!(
                    x.data()[PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN
                            + PUSH_HDR_LEN
                            + PACKET_HDR_LEN
                            + PUSH_HDR_LEN
                            + origin2.len()],
                    origin2
                );
            }
            None => panic!(),
        }
        assert!(upload.flush_packet().is_none());
    }
}
