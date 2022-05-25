use std::{
    collections::{BTreeMap, VecDeque},
    time::{self, Duration},
};

use crate::{
    protocols::yatcp::{
        frag_hdr::{FragCommand, FragHeaderBuilder, ACK_HDR_LEN, PUSH_HDR_LEN},
        packet_hdr::{PacketHeaderBuilder, PACKET_HDR_LEN},
    },
    utils::{self, BufPasta, BufWtr, FastRetransmissionWnd, Seq, SubBufWtr},
};

use super::{sending_frag::SendingFrag, SetUploadState};

const ALPHA: f64 = 1.0 / 8.0;
const MAX_RTO_MS: u64 = 60_000;
const DEFAULT_RTO_MS: u64 = 3_000; // make it bigger to avoid RTO floods
const MIN_RTO_MS: u64 = 100;
const RATIO_RTT_RTO: f64 = 1.5;
const NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT: usize = 0;
static MAX_RTO: time::Duration = Duration::from_millis(MAX_RTO_MS);
static DEFAULT_RTO: time::Duration = Duration::from_millis(DEFAULT_RTO_MS);
static MIN_RTO: time::Duration = Duration::from_millis(MIN_RTO_MS);

pub struct YatcpUpload {
    // modified by `append_frags_to`
    to_send_queue: VecDeque<utils::BufRdr>,
    sending_queue: BTreeMap<Seq, SendingFrag>,
    to_ack_queue: VecDeque<Seq>,
    next_seq_to_send: Seq,

    // modified by setters
    local_receiving_queue_free_len: usize,
    local_next_seq_to_receive: Seq,
    remote_rwnd: u16,
    fast_retransmission_wnd: FastRetransmissionWnd,

    // stat
    stat: LocalStat,
}

pub struct YatcpUploadBuilder {
    pub local_receiving_queue_len: usize,
}

impl YatcpUploadBuilder {
    pub fn build(self) -> YatcpUpload {
        let this = YatcpUpload {
            to_send_queue: VecDeque::new(),
            sending_queue: BTreeMap::new(),
            to_ack_queue: VecDeque::new(),
            local_receiving_queue_free_len: self.local_receiving_queue_len,
            local_next_seq_to_receive: Seq::from_u32(0),
            next_seq_to_send: Seq::from_u32(0),
            remote_rwnd: 0,
            stat: LocalStat {
                srtt: None,
                retransmissions: 0,
                rto_hits: 0,
                fast_retransmissions: 0,
            },
            fast_retransmission_wnd: FastRetransmissionWnd::new(
                NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT,
            ),
        };
        this.check_rep();
        this
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidState,
}

impl YatcpUpload {
    #[inline]
    fn check_rep(&self) {
        if !self.to_send_queue.is_empty() {
            assert!(!self.to_send_queue.front().unwrap().is_empty());
        }
    }

    pub fn stat(&self) -> Stat {
        Stat {
            srtt: self.stat.srtt,
            retransmissions: self.stat.retransmissions,
            rto_hits: self.stat.rto_hits,
            fast_retransmissions: self.stat.fast_retransmissions,
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
        assert!(PACKET_HDR_LEN + PUSH_HDR_LEN + 1 <= wtr.back_len());

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

        // retransmission
        // write push from sending
        let rto = self.rto();
        for (&seq, frag) in self.sending_queue.iter_mut() {
            if !(PUSH_HDR_LEN + 1 <= wtr.back_len()) {
                self.check_rep();
                assert!(!wtr.is_empty());
                return;
            }
            if !(frag.body().len() + PUSH_HDR_LEN <= wtr.back_len()) {
                continue;
            }
            if !(frag.is_timeout(&rto)
                // fast retransmit
                // TODO: test cases
                || self.fast_retransmission_wnd.contains(seq))
            {
                continue;
            }
            let hdr = FragHeaderBuilder {
                seq,
                cmd: FragCommand::Push {
                    len: frag.body().len() as u32,
                },
            }
            .build()
            .unwrap();
            let bytes = hdr.to_bytes();
            assert_eq!(bytes.len(), PUSH_HDR_LEN);
            wtr.append(&bytes).unwrap();
            frag.body().append_to(wtr).unwrap();
            frag.to_retransmit(); // test case: `test_rto_once`
            if self.fast_retransmission_wnd.contains(seq) {
                self.fast_retransmission_wnd.retransmitted(seq); // test case: `test_fast_retransmit`
                self.stat.fast_retransmissions += 1;
            } else {
                self.stat.rto_hits += 1;
            }
            self.stat.retransmissions += 1;
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
            assert!(frag_body.len() > 0);

            let seq = self.next_seq_to_send;
            self.next_seq_to_send.increment();
            let frag = SendingFrag::new(frag_body);
            // write the frag to output buffer
            let hdr = FragHeaderBuilder {
                seq,
                cmd: FragCommand::Push {
                    len: frag.body().len() as u32,
                },
            }
            .build()
            .unwrap();
            let bytes = hdr.to_bytes();
            assert_eq!(bytes.len(), PUSH_HDR_LEN);
            wtr.append(&bytes).unwrap();
            frag.body().append_to(wtr).unwrap();
            // register the frag to sending_queue
            self.sending_queue.insert(seq, frag);
        }

        self.check_rep();
        assert!(!wtr.is_empty());
        return;
    }

    #[inline]
    pub fn rto(&self) -> time::Duration {
        match self.stat.srtt {
            Some(srtt) => {
                let rto = srtt.mul_f64(RATIO_RTT_RTO);
                let rto = Duration::min(rto, MAX_RTO);
                let rto = Duration::max(rto, MIN_RTO);
                rto
            }
            None => DEFAULT_RTO,
        }
    }

    #[inline]
    fn set_remote_rwnd(&mut self, wnd: u16) {
        self.remote_rwnd = wnd;
        self.check_rep();
    }

    #[inline]
    fn set_local_next_seq_to_receive(&mut self, local_next_seq_to_receive: Seq) {
        self.local_next_seq_to_receive = local_next_seq_to_receive;
        self.check_rep();
    }

    #[inline]
    fn add_remote_seq_to_ack(&mut self, remote_seq_to_ack: Seq) {
        self.to_ack_queue.push_back(remote_seq_to_ack);
        self.check_rep();
    }

    #[inline]
    fn set_acked_local_seq(&mut self, acked_local_seq: Seq) {
        // remove the selected sequence
        if let Some(frag) = self.sending_queue.remove(&acked_local_seq) {
            if !frag.is_retransmitted() {
                // set smooth RTT
                let frag_rtt = frag.since_last_sent();
                match self.stat.srtt {
                    Some(srtt) => {
                        let new_srtt = srtt.mul_f64(1.0 - ALPHA) + frag_rtt.mul_f64(ALPHA);
                        self.stat.srtt = Some(new_srtt);
                    }
                    None => self.stat.srtt = Some(frag_rtt),
                }
            }
            // else, `last_seen` might just been modified, letting `srtt` become smaller
        }
        self.check_rep();
    }

    #[inline]
    fn remove_sending_before(&mut self, remote_nack: Seq) {
        let mut to_removes = Vec::new();
        for (&seq, _) in &self.sending_queue {
            if seq < remote_nack {
                to_removes.push(seq);
            } else {
                break;
            }
        }
        for to_remove in to_removes {
            self.sending_queue.remove(&to_remove);
        }
        self.check_rep();
    }

    #[inline]
    fn set_receiving_queue_free_len(&mut self, local_receiving_queue_free_len: usize) {
        self.local_receiving_queue_free_len = local_receiving_queue_free_len;
    }

    #[inline]
    pub fn set_state(&mut self, delta: SetUploadState) -> Result<(), Error> {
        for &acked_local_seq in &delta.acked_local_seqs {
            if acked_local_seq == delta.remote_nack {
                return Err(Error::InvalidState);
            }
        }

        self.set_remote_rwnd(delta.remote_rwnd);
        self.set_local_next_seq_to_receive(delta.local_next_seq_to_receive);
        self.set_receiving_queue_free_len(delta.local_receiving_queue_free_len);
        let mut max_acked_local_seq = None;
        for acked_local_seq in delta.acked_local_seqs {
            self.set_acked_local_seq(acked_local_seq);
            max_acked_local_seq = Some(match max_acked_local_seq {
                Some(x) => Seq::max(x, acked_local_seq),
                None => acked_local_seq,
            });
        }
        self.remove_sending_before(delta.remote_nack); // must after `set_acked_local_seq`s
                                                       // to retransmit all sequences before the largest out-of-order sequence
        if let Some(x) = max_acked_local_seq {
            if delta.remote_nack < x {
                self.fast_retransmission_wnd
                    .try_set_boundaries(delta.remote_nack..x);
            }
        }

        for remote_seq_to_ack in delta.remote_seqs_to_ack {
            self.add_remote_seq_to_ack(remote_seq_to_ack);
        }
        Ok(())
    }
}

struct LocalStat {
    srtt: Option<time::Duration>,
    retransmissions: u64,
    rto_hits: u64,
    fast_retransmissions: u64,
}

#[derive(Debug, PartialEq)]
pub struct Stat {
    pub srtt: Option<time::Duration>,
    pub retransmissions: u64,
    pub rto_hits: u64,
    pub fast_retransmissions: u64,
}

#[cfg(test)]
mod tests {
    use std::thread;

    use crate::{
        protocols::yatcp::{frag_hdr::PUSH_HDR_LEN, packet_hdr::PACKET_HDR_LEN},
        utils::{BufRdr, BufWtr, OwnedBufWtr, Seq},
        yatcp::{
            yatcp_upload::{
                YatcpUploadBuilder, NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT,
            },
            SetUploadState,
        },
    };

    const MTU: usize = 512;

    #[test]
    fn test_empty() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
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
        assert_eq!(upload.next_seq_to_send.to_u32(), 1);
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }

    #[test]
    fn test_few_many() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
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
        assert_eq!(upload.next_seq_to_send.to_u32(), 1);
        packet.reset_data(0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(!is_written);
        assert_eq!(upload.next_seq_to_send.to_u32(), 1);

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
        assert_eq!(upload.next_seq_to_send.to_u32(), 2);
        packet.reset_data(0);
        assert!(!upload.append_packet_to_and_if_written(&mut packet));
    }

    #[test]
    fn test_many_few() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
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
        assert_eq!(upload.next_seq_to_send.to_u32(), 1);
        assert_eq!(upload.sending_queue.len(), 1);

        upload.set_acked_local_seq(Seq::from_u32(0));

        assert_eq!(upload.sending_queue.len(), 0);
    }

    #[test]
    #[should_panic]
    fn test_not_enough_space_for_push() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
        }
        .build();
        upload.set_remote_rwnd(2);

        let origin1 = vec![0, 1, 2];
        {
            let rdr = BufRdr::from_bytes(origin1);
            upload.to_send(rdr);
        }
        let mut packet = OwnedBufWtr::new(PACKET_HDR_LEN + PUSH_HDR_LEN, 0);
        upload.append_packet_to_and_if_written(&mut packet);
    }

    #[test]
    fn test_rto_once() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
        }
        .build();
        upload.set_remote_rwnd(2);

        let origin1 = vec![0, 1, 2];
        {
            let rdr = BufRdr::from_bytes(origin1);
            upload.to_send(rdr);
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(is_written);

        thread::sleep(upload.rto());

        packet.reset_data(0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        // last_seen of the fragment is refreshed
        assert!(is_written);

        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        // the fragment doesn't timeout
        assert!(!is_written);
    }

    #[test]
    fn test_fast_retransmit() {
        let mut upload = YatcpUploadBuilder {
            local_receiving_queue_len: 0,
        }
        .build();
        upload.set_remote_rwnd(2);

        let origin1 = vec![0, 1, 2];
        {
            let rdr = BufRdr::from_bytes(origin1);
            upload.to_send(rdr);
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(is_written);

        let origin2 = vec![3];
        {
            let rdr = BufRdr::from_bytes(origin2);
            upload.to_send(rdr);
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let is_written = upload.append_packet_to_and_if_written(&mut packet);
        assert!(is_written);

        for i in 0..NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT + 1 {
            let state = SetUploadState {
                remote_rwnd: 99,
                remote_nack: Seq::from_u32(0),
                local_next_seq_to_receive: Seq::from_u32(0),
                remote_seqs_to_ack: vec![],
                acked_local_seqs: vec![Seq::from_u32(1)],
                local_receiving_queue_free_len: 1,
            };
            upload.set_state(state).unwrap();

            packet.reset_data(0);
            let is_written = upload.append_packet_to_and_if_written(&mut packet);

            if i == NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT {
                assert!(is_written);
            } else {
                assert!(!is_written);
            }
        }
    }
}
