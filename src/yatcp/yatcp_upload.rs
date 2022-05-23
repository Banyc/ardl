use std::{
    collections::{BTreeMap, LinkedList},
    time,
};

use crate::{
    protocols::yatcp::{
        frag_hdr::{FragCommand, FragHeaderBuilder, ACK_HDR_LEN, PUSH_HDR_LEN},
        packet_hdr::{PacketHeaderBuilder, PACKET_HDR_LEN},
    },
    utils::{self, BufPasta, BufRdr, BufWtr, SubBufWtr},
};

pub struct YatcpUpload {
    to_send_queue: LinkedList<utils::BufRdr>,
    sending_queue: BTreeMap<u32, SendingFrag>,
    to_ack_queue: LinkedList<u32>,
    local_receiving_queue_free_len: usize,
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
            local_receiving_queue_free_len: self.receiving_queue_len,
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
    }

    pub fn to_send(&mut self, buf: utils::OwnedBufWtr) {
        if buf.is_empty() {
            return;
        }
        let rdr = BufRdr::new(buf);
        self.to_send_queue.push_back(rdr);
        self.check_rep();
    }

    pub fn append_packet_to_and_if_written(&mut self, wtr: &mut impl BufWtr) -> bool {
        assert!(PACKET_HDR_LEN + ACK_HDR_LEN <= wtr.back_len());
        assert!(PACKET_HDR_LEN + PUSH_HDR_LEN < wtr.back_len());

        let mut subwtr = SubBufWtr::new(wtr.back_free_space(), PACKET_HDR_LEN);

        self.append_frags_to(&mut subwtr);
        let is_written = !subwtr.is_empty();

        if is_written {
            // packet header
            let hdr = PacketHeaderBuilder {
                wnd: self.local_receiving_queue_free_len as u16,
                nack: self.next_seq_to_receive,
            }
            .build()
            .unwrap();
            let bytes = hdr.to_bytes();
            assert_eq!(bytes.len(), PACKET_HDR_LEN);
            subwtr.prepend(&bytes).unwrap();

            let subwtr_len = subwtr.data_len();

            wtr.grow_back(subwtr_len).unwrap();
        }
        is_written
    }

    #[inline]
    fn append_frags_to(&mut self, wtr: &mut impl BufWtr) {
        if self.to_ack_queue.is_empty() && self.to_send_queue.is_empty() {
            assert!(wtr.is_empty());
            return;
        }

        // piggyback ack
        loop {
            if !(ACK_HDR_LEN <= wtr.back_len()) {
                self.check_rep();
                assert!(!wtr.is_empty());
                return;
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
            if !(PUSH_HDR_LEN < wtr.back_len()) {
                self.check_rep();
                assert!(!wtr.is_empty());
                return;
            }
            if !frag.is_timeout(&self.re_tx_timeout) {
                continue;
            }
            if !(frag.body.len() + PUSH_HDR_LEN <= wtr.back_len()) {
                continue;
            }
            let hdr = FragHeaderBuilder {
                seq: frag.seq,
                cmd: FragCommand::Push {
                    len: frag.body.len() as u32,
                },
            }
            .build()
            .unwrap();
            wtr.append(&hdr.to_bytes()).unwrap();
            frag.body.append_to(wtr).unwrap();
        }

        if !(PUSH_HDR_LEN < wtr.back_len()) {
            self.check_rep();
            assert!(!wtr.is_empty());
            return;
        }
        if !(self.sending_queue.len() < u16::max(self.remote_rwnd, 1) as usize) {
            self.check_rep();
            if wtr.is_empty() {
                assert!(wtr.is_empty());
                return;
            } else {
                assert!(!wtr.is_empty());
                return;
            }
        }

        // move data from to_send queue to sending queue and output those data
        {
            // get as many bytes from to_send_queue
            // call those bytes "frag"
            let frag_body_limit = wtr.back_len() - PUSH_HDR_LEN;
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
            let hdr = FragHeaderBuilder {
                seq: frag.seq,
                cmd: FragCommand::Push {
                    len: frag.body.len() as u32,
                },
            }
            .build()
            .unwrap();
            wtr.append(&hdr.to_bytes()).unwrap();
            frag.body.append_to(wtr).unwrap();
            // register the frag to sending_queue
            self.sending_queue.insert(frag.seq, frag);
        }

        self.check_rep();
        assert!(!wtr.is_empty());
        return;
    }

    #[inline]
    pub fn set_remote_rwnd(&mut self, wnd: u16) {
        self.remote_rwnd = wnd;
        self.check_rep();
    }

    #[inline]
    pub fn set_next_seq_to_receive(&mut self, nack: u32) {
        self.next_seq_to_receive = nack;
        self.sending_queue.retain(|&seq, _| !(seq < nack));
        self.check_rep();
    }

    #[inline]
    pub fn add_seq_to_ack(&mut self, seq_to_ack: u32) {
        self.to_ack_queue.push_back(seq_to_ack);
        self.check_rep();
    }

    #[inline]
    pub fn remove_sending(&mut self, ack: u32) {
        self.sending_queue.remove(&ack);
    }

    #[inline]
    pub fn set_receiving_queue_free_len(&mut self, local_receiving_queue_free_len: usize) {
        self.local_receiving_queue_free_len = local_receiving_queue_free_len;
    }
}

#[cfg(test)]
mod tests {
    use std::time;

    use crate::{
        protocols::yatcp::{frag_hdr::PUSH_HDR_LEN, packet_hdr::PACKET_HDR_LEN},
        utils::{BufWtr, OwnedBufWtr},
        yatcp::yatcp_upload::YatcpUploadBuilder,
    };

    const MTU: usize = 512;

    #[test]
    fn test_empty() {
        let mut upload = YatcpUploadBuilder {
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let buf = OwnedBufWtr::new(MTU / 2, 0);
        upload.to_send(buf);
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(!is_written);
    }

    #[test]
    fn test_few_1() {
        let mut upload = YatcpUploadBuilder {
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin = vec![0, 1, 2];
        buf.append(&origin).unwrap();
        upload.to_send(buf);
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        match is_written {
            true => {
                assert_eq!(
                    packet.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + origin.len()
                );
                assert_eq!(packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..], origin);
            }
            false => panic!(),
        }
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }

    #[test]
    fn test_few_2() {
        let mut upload = YatcpUploadBuilder {
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        upload.to_send(buf);
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![3, 4];
        buf.append(&origin2).unwrap();
        upload.to_send(buf);
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        match is_written {
            true => {
                assert_eq!(
                    packet.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len() + origin2.len()
                );
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()],
                    origin1
                );
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()..],
                    origin2
                );
            }
            false => panic!(),
        }
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }

    #[test]
    fn test_few_many() {
        let mut upload = YatcpUploadBuilder {
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        upload.to_send(buf);
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin2 = vec![3; MTU];
        buf.append(&origin2).unwrap();
        upload.to_send(buf);
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        match is_written {
            true => {
                assert_eq!(packet.data_len(), MTU);
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()],
                    origin1
                );
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()..],
                    origin2[..MTU - PACKET_HDR_LEN - PUSH_HDR_LEN - origin1.len()]
                );
            }
            false => panic!(),
        }
        packet.reset_data(0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(!is_written);

        upload.set_remote_rwnd(10);

        packet.reset_data(0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        match is_written {
            true => {
                assert_eq!(
                    packet.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN + origin1.len()
                );
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..],
                    origin2[MTU - PACKET_HDR_LEN - PUSH_HDR_LEN - origin1.len()..]
                );
            }
            false => panic!(),
        }
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }

    #[test]
    fn test_many_few() {
        let mut upload = YatcpUploadBuilder {
            receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin1 = vec![3; MTU];
        buf.append(&origin1).unwrap();
        upload.to_send(buf);
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![0, 1, 2];
        buf.append(&origin2).unwrap();
        upload.to_send(buf);
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        // packet: _hdr hdr mtu-_hdr-hdr
        // origin:          1[0..mtu-_hdr-hdr]
        match is_written {
            true => {
                assert_eq!(packet.data_len(), MTU);
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..],
                    origin1[..MTU - PACKET_HDR_LEN - PUSH_HDR_LEN],
                );
            }
            false => panic!(),
        }
        packet.reset_data(0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(!is_written);

        upload.set_remote_rwnd(10);

        packet.reset_data(0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        // packet: _hdr hdr _hdr+hdr             3
        // origin:          1[mtu-_hdr-hdr..mtu] 2[0..3]
        match is_written {
            true => {
                assert_eq!(
                    packet.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN + origin2.len()
                );
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN],
                    origin1[MTU - PACKET_HDR_LEN - PUSH_HDR_LEN..]
                );
                assert_eq!(
                    packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN + PACKET_HDR_LEN + PUSH_HDR_LEN
                        ..PACKET_HDR_LEN
                            + PUSH_HDR_LEN
                            + PACKET_HDR_LEN
                            + PUSH_HDR_LEN
                            + origin2.len()],
                    origin2
                );
            }
            false => panic!(),
        }
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }
}
