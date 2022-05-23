use std::{
    collections::{BTreeMap, VecDeque},
    time,
};

use crate::{
    protocols::yatcp::{
        frag_hdr::{FragCommand, FragHeaderBuilder, ACK_HDR_LEN, PUSH_HDR_LEN},
        packet_hdr::{PacketHeaderBuilder, PACKET_HDR_LEN},
    },
    utils::{self, BufPasta, BufWtr, SubBufWtr},
};

use super::SetUploadStates;

pub struct YatcpUpload {
    // modified by `append_frags_to`
    to_send_queue: VecDeque<utils::BufRdr>,
    sending_queue: BTreeMap<u32, SendingFrag>,
    to_ack_queue: VecDeque<u32>,
    next_seq_to_send: u32,

    // modified by setters
    local_receiving_queue_free_len: usize,
    local_next_seq_to_receive: u32,
    remote_rwnd: u16,

    // immutable
    re_tx_timeout: time::Duration,
}

pub struct YatcpUploadBuilder {
    pub local_receiving_queue_len: usize,
    pub re_tx_timeout: time::Duration,
}

impl YatcpUploadBuilder {
    pub fn build(self) -> YatcpUpload {
        let this = YatcpUpload {
            to_send_queue: VecDeque::new(),
            sending_queue: BTreeMap::new(),
            to_ack_queue: VecDeque::new(),
            local_receiving_queue_free_len: self.local_receiving_queue_len,
            local_next_seq_to_receive: 0,
            re_tx_timeout: self.re_tx_timeout,
            next_seq_to_send: 0,
            remote_rwnd: 0,
        };
        this.check_rep();
        this
    }
}

struct SendingFrag {
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

    pub fn to_send(&mut self, rdr: utils::BufRdr) {
        if rdr.is_empty() {
            return;
        }
        self.to_send_queue.push_back(rdr);
        self.check_rep();
    }

    pub fn append_packet_to_and_if_written(&mut self, wtr: &mut impl BufWtr) -> bool {
        assert!(PACKET_HDR_LEN + ACK_HDR_LEN <= wtr.back_len());
        assert!(PACKET_HDR_LEN + PUSH_HDR_LEN < wtr.back_len());

        let mut sub_wtr = SubBufWtr::new(wtr.back_free_space(), PACKET_HDR_LEN);

        self.append_frags_to(&mut sub_wtr);
        let is_written = !sub_wtr.is_empty();

        if is_written {
            // packet header
            let hdr = PacketHeaderBuilder {
                rwnd: self.local_receiving_queue_free_len as u16,
                nack: self.local_next_seq_to_receive,
            }
            .build()
            .unwrap();
            let bytes = hdr.to_bytes();
            assert_eq!(bytes.len(), PACKET_HDR_LEN);
            sub_wtr.prepend(&bytes).unwrap();

            let sub_wtr_len = sub_wtr.data_len();
            wtr.grow_back(sub_wtr_len).unwrap();
        }
        is_written
    }

    #[inline]
    fn append_frags_to(&mut self, wtr: &mut impl BufWtr) {
        if self.to_ack_queue.is_empty()
            && self.sending_queue.is_empty()
            && self.to_send_queue.is_empty()
        {
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
            let bytes = hdr.to_bytes();
            assert_eq!(bytes.len(), ACK_HDR_LEN);
            wtr.append(&bytes).unwrap();
        }

        // write push from sending
        for (&seq, frag) in self.sending_queue.iter() {
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
                seq,
                cmd: FragCommand::Push {
                    len: frag.body.len() as u32,
                },
            }
            .build()
            .unwrap();
            let bytes = hdr.to_bytes();
            assert_eq!(bytes.len(), PUSH_HDR_LEN);
            wtr.append(&bytes).unwrap();
            frag.body.append_to(wtr).unwrap();
        }

        if self.to_send_queue.is_empty() {
            self.check_rep();
            return;
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
                let buf = rdr.try_slice(frag_body_limit - frag_body.len()).unwrap();
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
                body: frag_body,
                last_seen: time::Instant::now(),
            };
            // write the frag to output buffer
            let hdr = FragHeaderBuilder {
                seq,
                cmd: FragCommand::Push {
                    len: frag.body.len() as u32,
                },
            }
            .build()
            .unwrap();
            let bytes = hdr.to_bytes();
            assert_eq!(bytes.len(), PUSH_HDR_LEN);
            wtr.append(&bytes).unwrap();
            frag.body.append_to(wtr).unwrap();
            // register the frag to sending_queue
            self.sending_queue.insert(seq, frag);
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
    pub fn set_local_next_seq_to_receive(&mut self, local_next_seq_to_receive: u32) {
        self.local_next_seq_to_receive = local_next_seq_to_receive;
        self.check_rep();
    }

    #[inline]
    pub fn add_remote_seq_to_ack(&mut self, remote_seq_to_ack: u32) {
        self.to_ack_queue.push_back(remote_seq_to_ack);
        self.check_rep();
    }

    #[inline]
    pub fn remove_sending(&mut self, acked_local_seq: u32) {
        self.sending_queue.remove(&acked_local_seq);
        self.check_rep();
    }

    #[inline]
    pub fn remove_sending_before(&mut self, remote_nack: u32) {
        self.sending_queue.retain(|&seq, _| !(seq < remote_nack));
        self.check_rep();
    }

    #[inline]
    pub fn set_receiving_queue_free_len(&mut self, local_receiving_queue_free_len: usize) {
        self.local_receiving_queue_free_len = local_receiving_queue_free_len;
    }

    #[inline]
    pub fn set_states(&mut self, delta: SetUploadStates) {
        self.set_remote_rwnd(delta.remote_rwnd);
        self.set_local_next_seq_to_receive(delta.local_next_seq_to_receive);
        self.remove_sending_before(delta.remote_nack);
        self.set_receiving_queue_free_len(delta.local_receiving_queue_free_len);
        for acked_local_seq in delta.acked_local_seqs {
            self.remove_sending(acked_local_seq);
        }
        for remote_seq_to_ack in delta.remote_seqs_to_ack {
            self.add_remote_seq_to_ack(remote_seq_to_ack);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time;

    use crate::{
        protocols::yatcp::{frag_hdr::PUSH_HDR_LEN, packet_hdr::PACKET_HDR_LEN},
        utils::{BufRdr, BufWtr, OwnedBufWtr},
        yatcp::yatcp_upload::YatcpUploadBuilder,
    };

    const MTU: usize = 512;

    #[test]
    fn test_empty() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let buf = OwnedBufWtr::new(MTU / 2, 0);
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(!is_written);
    }

    #[test]
    fn test_few_1() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin = vec![0, 1, 2];
        buf.append(&origin).unwrap();
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
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
            local_receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![3, 4];
        buf.append(&origin2).unwrap();
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
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
        assert_eq!(upload.next_seq_to_send, 1);
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }

    #[test]
    fn test_few_many() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin2 = vec![3; MTU];
        buf.append(&origin2).unwrap();
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
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
        assert_eq!(upload.next_seq_to_send, 1);
        packet.reset_data(0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(!is_written);
        assert_eq!(upload.next_seq_to_send, 1);

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
        assert_eq!(upload.next_seq_to_send, 2);
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }

    #[test]
    fn test_many_few() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin1 = vec![3; MTU];
        buf.append(&origin1).unwrap();
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![0, 1, 2];
        buf.append(&origin2).unwrap();
        let rdr = BufRdr::from_wtr(buf);
        upload.to_send(rdr);
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

    #[test]
    fn test_ack1() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        upload.set_remote_rwnd(2);

        let origin1 = vec![0, 1, 2];
        {
            let mut buf = OwnedBufWtr::new(MTU / 2, 0);
            buf.append(&origin1).unwrap();
            let rdr = BufRdr::from_wtr(buf);
            upload.to_send(rdr);
        }
        let origin2 = vec![3, 4];
        {
            let mut buf = OwnedBufWtr::new(MTU / 2, 0);
            buf.append(&origin2).unwrap();
            let rdr = BufRdr::from_wtr(buf);
            upload.to_send(rdr);
        }
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
        assert_eq!(upload.next_seq_to_send, 1);
        assert_eq!(upload.sending_queue.len(), 1);

        upload.remove_sending(0);

        assert_eq!(upload.sending_queue.len(), 0);
    }
}
