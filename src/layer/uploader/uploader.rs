use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
    time::{self, Duration},
};

use crate::{
    protocol::{
        frag::{Body, Frag, FragBuilder, FragCommand, ACK_HDR_LEN, PUSH_HDR_LEN},
        packet::PacketBuilder,
        packet_hdr::{PacketHeaderBuilder, PACKET_HDR_LEN},
    },
    utils::{
        buf::{self, BufPasta, BufSlicerQue, BufWtr},
        FastRetransmissionWnd, Seq, Swnd,
    },
};

use super::{
    super::{IObserver, SetUploadState},
    SendingFrag,
};

const ALPHA: f64 = 1.0 / 8.0;
const MAX_RTO_MS: u64 = 60_000;
const DEFAULT_RTO_MS: u64 = 3_000; // make it bigger to avoid RTO floods
const MIN_RTO_MS: u64 = 100;
static MAX_RTO: time::Duration = Duration::from_millis(MAX_RTO_MS);
static DEFAULT_RTO: time::Duration = Duration::from_millis(DEFAULT_RTO_MS);
static MIN_RTO: time::Duration = Duration::from_millis(MIN_RTO_MS);

pub struct Uploader {
    // modified by `append_frags_to`
    to_send_queue: buf::BufSlicerQue,
    swnd: Swnd<SendingFrag>,
    to_ack_queue: VecDeque<Seq>,

    // modified by setters
    local_rwnd_size: usize,
    local_next_seq_to_receive: Seq,
    fast_retransmission_wnd: FastRetransmissionWnd,

    // stat
    stat: LocalStat,

    // const
    ratio_rto_to_one_rtt: f64,

    // unit tests
    disable_rto: bool,

    // observer
    on_send_available: Option<Weak<dyn IObserver + Send + Sync + 'static>>,
}

pub struct UploaderBuilder {
    pub local_recv_buf_len: usize,
    pub nack_duplicate_threshold_to_activate_fast_retransmit: usize,
    pub ratio_rto_to_one_rtt: f64,
    pub to_send_queue_len_cap: usize,
    pub swnd_size_cap: usize,
}

impl UploaderBuilder {
    #[must_use]
    pub fn build(self) -> Uploader {
        let this = Uploader {
            to_send_queue: BufSlicerQue::new(self.to_send_queue_len_cap),
            swnd: Swnd::new(self.swnd_size_cap),
            to_ack_queue: VecDeque::new(),
            local_rwnd_size: self.local_recv_buf_len,
            local_next_seq_to_receive: Seq::from_u32(0),
            stat: LocalStat {
                srtt: None,
                retransmissions: 0,
                rto_hits: 0,
                fast_retransmissions: 0,
                pushes: 0,
                acks: 0,
            },
            fast_retransmission_wnd: FastRetransmissionWnd::new(
                self.nack_duplicate_threshold_to_activate_fast_retransmit,
            ),
            ratio_rto_to_one_rtt: self.ratio_rto_to_one_rtt,
            disable_rto: false,
            on_send_available: None,
        };
        this.check_rep();
        this
    }

    #[must_use]
    pub fn default() -> UploaderBuilder {
        let builder = Self {
            local_recv_buf_len: u16::MAX as usize,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: 1024 * 64,
            swnd_size_cap: u16::MAX as usize,
        };
        builder
    }
}

#[derive(Debug)]
pub enum SetStateError {
    InvalidState,
}

#[derive(Debug)]
pub enum OutputError {
    NothingToOutput,
    BufferTooSmall,
}

pub struct SendError<T>(pub T);

impl Uploader {
    #[inline]
    fn check_rep(&self) {}

    #[must_use]
    pub fn stat(&self) -> Stat {
        Stat {
            srtt: self.stat.srtt,
            retransmissions: self.stat.retransmissions,
            rto_hits: self.stat.rto_hits,
            fast_retransmissions: self.stat.fast_retransmissions,
            pushes: self.stat.pushes,
            acks: self.stat.acks,
            next_seq_to_send: self.swnd.end(),
        }
    }

    pub fn set_on_send_available(
        &mut self,
        observer: Option<Weak<dyn IObserver + Send + Sync + 'static>>,
    ) {
        self.on_send_available = observer;
    }

    pub fn to_send(&mut self, slice: buf::BufSlice) -> Result<(), SendError<buf::BufSlice>> {
        let result = match self.to_send_queue.push_back(slice) {
            Ok(_) => Ok(()),
            Err(e) => Err(SendError(e.0)),
        };
        result
    }

    pub fn output_packet(&mut self, wtr: &mut impl BufWtr) -> Result<(), OutputError> {
        let is_then_full = self.to_send_queue.is_full();
        self.append_packet_to(wtr)?;

        // callback when `to_send` is not full
        if let Some(x) = &self.on_send_available {
            let is_now_full = self.to_send_queue.is_full();
            match (is_then_full, is_now_full) {
                (true, false) => match x.upgrade() {
                    Some(x) => x.notify(),
                    None => (),
                },
                _ => (),
            }
        }

        self.check_rep();
        Ok(())
    }

    fn append_packet_to(&mut self, wtr: &mut impl BufWtr) -> Result<(), OutputError> {
        if !(PACKET_HDR_LEN + ACK_HDR_LEN <= wtr.back_len()) {
            self.check_rep();
            return Err(OutputError::BufferTooSmall);
        }
        if !(PACKET_HDR_LEN + PUSH_HDR_LEN + 1 <= wtr.back_len()) {
            self.check_rep();
            return Err(OutputError::BufferTooSmall);
        }

        let frags = self.collect_frags(wtr.back_len() - PACKET_HDR_LEN);

        if !frags.is_empty() {
            // packet header
            let hdr = PacketHeaderBuilder {
                rwnd: self.local_rwnd_size as u16,
                nack: self.local_next_seq_to_receive,
            }
            .build()
            .unwrap();

            let packet = PacketBuilder { hdr, frags }.build().unwrap();

            packet.append_to(wtr).unwrap();
            self.check_rep();
            Ok(())
        } else {
            self.check_rep();
            Err(OutputError::NothingToOutput)
        }
    }

    #[inline]
    fn collect_frags(&mut self, mut space: usize) -> Vec<Frag> {
        let mut frags = Vec::new();
        if self.to_ack_queue.is_empty() && self.swnd.is_empty() && self.to_send_queue.is_empty() {
            assert!(frags.is_empty());
            return frags;
        }

        // piggyback ack
        loop {
            if !(ACK_HDR_LEN <= space) {
                self.check_rep();
                assert!(!frags.is_empty());
                return frags;
            }
            let ack = match self.to_ack_queue.pop_front() {
                Some(ack) => ack,
                None => break,
            };
            let frag = FragBuilder {
                seq: ack,
                cmd: FragCommand::Ack,
            }
            .build()
            .unwrap();
            space -= frag.len();
            frags.push(frag);
            self.stat.acks += 1;
        }

        // retransmission
        // write pushes from sending
        let rto = self.rto();
        for (&seq, push) in self.swnd.iter_mut() {
            if !(PUSH_HDR_LEN + 1 <= space) {
                self.check_rep();
                assert!(!frags.is_empty());
                return frags;
            }
            if !(push.body().len() + PUSH_HDR_LEN <= space) {
                continue;
            }
            let is_timeout = push.is_timeout(&rto);
            let if_fast_retransmit = self.fast_retransmission_wnd.contains(seq);
            if !(
                // fast retransmit
                // TODO: test cases
                (!self.disable_rto && is_timeout) || if_fast_retransmit
            ) {
                continue;
            }
            let frag = FragBuilder {
                seq,
                cmd: FragCommand::Push {
                    body: Body::Pasta(Arc::clone(push.body())),
                },
            }
            .build()
            .unwrap();
            space -= frag.len();
            frags.push(frag);
            push.to_retransmit(); // test case: `test_rto_once`
            if if_fast_retransmit {
                self.fast_retransmission_wnd.retransmitted(seq); // test case: `test_fast_retransmit`
                self.stat.fast_retransmissions += 1;
            }
            if is_timeout {
                self.stat.rto_hits += 1;
            }
            self.stat.retransmissions += 1;
            self.stat.pushes += 1;
        }

        if self.to_send_queue.is_empty() {
            self.check_rep();
            return frags;
        }
        if !(PUSH_HDR_LEN < space) {
            self.check_rep();
            assert!(!frags.is_empty());
            return frags;
        }
        if self.swnd.is_full() {
            self.check_rep();
            return frags;
        }

        // move data from to_send queue to sending queue and output those data
        {
            // get as many bytes from to_send_queue
            // call those bytes "frag"
            let frag_body_limit = space - PUSH_HDR_LEN;
            assert!(frag_body_limit != 0);
            let mut body = BufPasta::new();
            while !self.to_send_queue.is_empty() {
                let free_space = frag_body_limit - body.len();
                if free_space == 0 {
                    break;
                }
                let buf = self.to_send_queue.slice_front(free_space).unwrap();
                body.append(buf);
            }
            assert!(body.len() <= frag_body_limit);
            assert!(body.len() > 0);

            let push = SendingFrag::new(Arc::new(body));

            // write the frag to output buffer
            let frag = FragBuilder {
                seq: self.swnd.end(),
                cmd: FragCommand::Push {
                    body: Body::Pasta(Arc::clone(push.body())),
                },
            }
            .build()
            .unwrap();
            assert!(frag.len() <= space);
            frags.push(frag);

            // register the frag to swnd
            self.swnd.push_back(push);

            self.stat.pushes += 1;
        }

        self.check_rep();
        assert!(!frags.is_empty());
        return frags;
    }

    #[must_use]
    #[inline]
    pub fn rto(&self) -> time::Duration {
        match self.stat.srtt {
            Some(srtt) => {
                let rto = srtt.mul_f64(self.ratio_rto_to_one_rtt);
                let rto = Duration::min(rto, MAX_RTO);
                let rto = Duration::max(rto, MIN_RTO);
                rto
            }
            None => DEFAULT_RTO,
        }
    }

    #[inline]
    fn set_remote_rwnd_size(&mut self, wnd: u16) {
        self.swnd.set_remote_rwnd_size(wnd as usize);
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
        if let Some(frag) = self.swnd.remove(&acked_local_seq) {
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
        self.swnd.remove_before(remote_nack);
        self.check_rep();
    }

    #[inline]
    fn set_local_rwnd_size(&mut self, local_rwnd_size: usize) {
        self.local_rwnd_size = local_rwnd_size;
    }

    #[inline]
    pub fn set_state(&mut self, delta: SetUploadState) -> Result<(), SetStateError> {
        for &acked_local_seq in &delta.acked_local_seqs {
            if acked_local_seq == delta.remote_nack {
                return Err(SetStateError::InvalidState);
            }
        }

        self.set_remote_rwnd_size(delta.remote_rwnd_size);
        self.set_local_next_seq_to_receive(delta.local_next_seq_to_receive);
        self.set_local_rwnd_size(delta.local_rwnd_size);
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
    pushes: u64,
    acks: u64,
}

#[derive(Debug, PartialEq)]
pub struct Stat {
    pub srtt: Option<time::Duration>,
    pub retransmissions: u64,
    pub rto_hits: u64,
    pub fast_retransmissions: u64,
    pub pushes: u64,
    pub acks: u64,
    pub next_seq_to_send: Seq,
}

#[cfg(test)]
mod tests {
    use std::thread;

    use crate::{
        layer::{uploader::UploaderBuilder, SetUploadState},
        protocol::{
            frag::{ACK_HDR_LEN, PUSH_HDR_LEN},
            packet_hdr::PACKET_HDR_LEN,
        },
        utils::{
            buf::{BufSlice, BufWtr, OwnedBufWtr},
            Seq,
        },
    };

    use super::OutputError;

    const MTU: usize = 512;

    #[test]
    fn test_empty() {
        let mut upload = UploaderBuilder::default().build();
        let buf = OwnedBufWtr::new(MTU / 2, 0);
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(!result.is_ok());
    }

    #[test]
    fn test_few_1() {
        let mut upload = UploaderBuilder::default().build();
        upload.disable_rto = true;
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin = vec![0, 1, 2];
        buf.append(&origin).unwrap();
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        assert!(!upload.to_send_queue.is_empty());
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        match result.is_ok() {
            true => {
                assert!(upload.to_send_queue.is_empty());
                assert_eq!(
                    packet.data_len(),
                    PACKET_HDR_LEN + PUSH_HDR_LEN + origin.len()
                );
                assert_eq!(packet.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..], origin);
            }
            false => panic!(),
        }
        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);
        assert!(!result.is_ok());
    }

    #[test]
    fn test_few_2() {
        let mut upload = UploaderBuilder::default().build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![3, 4];
        buf.append(&origin2).unwrap();
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        match result.is_ok() {
            true => {
                assert!(upload.to_send_queue.is_empty());
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
        assert_eq!(upload.swnd.end().to_u32(), 1);
        packet.reset_data(0);
        assert!(upload.output_packet(&mut packet).is_err());
    }

    #[test]
    fn test_few_many() {
        let mut upload = UploaderBuilder::default().build();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin2 = vec![3; MTU];
        buf.append(&origin2).unwrap();
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        match result.is_ok() {
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
        assert_eq!(upload.swnd.end().to_u32(), 1);
        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);
        assert!(!result.is_ok());
        assert_eq!(upload.swnd.end().to_u32(), 1);

        upload.set_remote_rwnd_size(10);

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);
        match result.is_ok() {
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
        assert_eq!(upload.swnd.end().to_u32(), 2);
        packet.reset_data(0);
        assert!(upload.output_packet(&mut packet).is_err());
    }

    #[test]
    fn test_many_few() {
        let mut upload = UploaderBuilder::default().build();
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin1 = vec![3; MTU];
        buf.append(&origin1).unwrap();
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![0, 1, 2];
        buf.append(&origin2).unwrap();
        let slice = BufSlice::from_wtr(buf);
        upload.to_send(slice).map_err(|_| ()).unwrap();
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        // packet: _hdr hdr mtu-_hdr-hdr
        // origin:          1[0..mtu-_hdr-hdr]
        match result.is_ok() {
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
        let result = upload.output_packet(&mut packet);
        assert!(!result.is_ok());

        upload.set_remote_rwnd_size(10);

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);
        // packet: _hdr hdr _hdr+hdr             3
        // origin:          1[mtu-_hdr-hdr..mtu] 2[0..3]
        match result.is_ok() {
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
        assert!(upload.output_packet(&mut packet).is_err());
    }

    #[test]
    fn test_ack1() {
        let mut upload = UploaderBuilder::default().build();
        upload.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let mut buf = OwnedBufWtr::new(MTU / 2, 0);
            buf.append(&origin1).unwrap();
            let slice = BufSlice::from_wtr(buf);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let origin2 = vec![3, 4];
        {
            let mut buf = OwnedBufWtr::new(MTU / 2, 0);
            buf.append(&origin2).unwrap();
            let slice = BufSlice::from_wtr(buf);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        match result.is_ok() {
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
        assert_eq!(upload.swnd.end().to_u32(), 1);
        assert_eq!(upload.swnd.size(), 1);

        upload.set_acked_local_seq(Seq::from_u32(0));

        assert_eq!(upload.swnd.size(), 0);
    }

    #[test]
    fn test_not_enough_space_for_push() {
        let mut upload = UploaderBuilder::default().build();
        upload.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(PACKET_HDR_LEN + PUSH_HDR_LEN, 0);
        match upload.output_packet(&mut packet) {
            Ok(_) => panic!(),
            Err(e) => match e {
                OutputError::NothingToOutput => panic!(),
                OutputError::BufferTooSmall => (),
            },
        }
    }

    #[test]
    fn test_rto_once() {
        let mut upload = UploaderBuilder::default().build();
        upload.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        thread::sleep(upload.rto());

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);
        // last_seen of the fragment is refreshed
        assert!(result.is_ok());

        let result = upload.output_packet(&mut packet);
        // the fragment doesn't timeout
        assert!(!result.is_ok());
    }

    #[test]
    fn test_fast_retransmit1() {
        let dup = 1;
        let mut upload = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();
        upload.disable_rto = true;
        upload.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        // nack is 0 by default. When receiving another same nack, the fast retransmission gets activated since now the dup count becomes 1

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq::from_u32(0),
            local_next_seq_to_receive: Seq::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq::from_u32(1)],
            local_rwnd_size: 1,
        };
        upload.set_state(state).unwrap();

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);

        assert!(result.is_ok());
    }

    #[test]
    fn test_fast_retransmit_no() {
        let dup = 0;
        let mut upload = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();
        upload.disable_rto = true;
        upload.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq::from_u32(1),
            local_next_seq_to_receive: Seq::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq::from_u32(0)],
            local_rwnd_size: 1,
        };
        upload.set_state(state).unwrap();

        // remote wants seq(1)
        // since no out-of-order acks from the remote, we don't retransmit any seq

        // 0   1
        // ack nack

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);

        assert!(!result.is_ok());
    }

    #[test]
    fn test_fast_retransmit2() {
        let dup = 0;
        let mut upload = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();
        upload.disable_rto = true;
        upload.set_remote_rwnd_size(99);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let origin3 = vec![4];
        {
            let slice = BufSlice::from_bytes(origin3);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq::from_u32(1),
            local_next_seq_to_receive: Seq::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq::from_u32(2)],
            local_rwnd_size: 1,
        };
        upload.set_state(state).unwrap();

        // remote acked seq(2) but still wants seq(1)
        // clearly, seq(2) is an out-of-order ack, everything before it should be retransmitted if DUP meets

        // 0  1    2
        //    nack ack

        // seq(0) is implicitly acked by nack(1)

        // dup count for nack(1): 0

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);

        assert!(result.is_ok());
    }

    #[test]
    fn test_fast_retransmit3() {
        let dup = 1;
        let mut upload = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();
        upload.disable_rto = true;
        upload.set_remote_rwnd_size(99);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let origin3 = vec![4];
        {
            let slice = BufSlice::from_bytes(origin3);
            upload.to_send(slice).map_err(|_| ()).unwrap();
        }
        let mut packet = OwnedBufWtr::new(MTU, 0);
        let result = upload.output_packet(&mut packet);
        assert!(result.is_ok());

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq::from_u32(1),
            local_next_seq_to_receive: Seq::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq::from_u32(2)],
            local_rwnd_size: 1,
        };
        upload.set_state(state).unwrap();

        // remote acked seq(2) but still wants seq(1)
        // clearly, seq(2) is an out-of-order ack, everything before it should be retransmitted if DUP meets

        // 0  1    2
        //    nack ack

        // seq(0) is implicitly acked by nack(1)

        // dup count for nack(1): 0

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);

        assert!(!result.is_ok());

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq::from_u32(1),
            local_next_seq_to_receive: Seq::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq::from_u32(2)],
            local_rwnd_size: 1,
        };
        upload.set_state(state).unwrap();

        // dup count for nack(1): 1

        packet.reset_data(0);
        let result = upload.output_packet(&mut packet);

        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_frags() {
        let mut uploader = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();
        // uploader.disable_rto = true;

        //           0  1  2  3
        // to_ack
        // swnd    ][
        // to_send  []

        uploader
            .to_send(BufSlice::from_bytes(vec![9, 8, 7]))
            .map_err(|_| ())
            .unwrap();

        //           0  1  2  3
        // to_ack
        // swnd    ][
        // to_send  [[9, 8, 7]]
        assert!(!uploader.to_send_queue.is_empty());
        assert!(uploader.swnd.is_empty());

        uploader
            .set_state(SetUploadState {
                remote_rwnd_size: 99,
                remote_nack: Seq::from_u32(2),
                local_next_seq_to_receive: Seq::from_u32(88),
                remote_seqs_to_ack: vec![Seq::from_u32(0), Seq::from_u32(1)],
                acked_local_seqs: Vec::new(),
                local_rwnd_size: 99,
            })
            .unwrap();

        //           0  1  2  3
        // to_ack    x  x
        // swnd    ][
        // to_send  [[9, 8, 7]]
        assert_eq!(uploader.to_ack_queue.len(), 2);

        let mut wtr = OwnedBufWtr::new(PACKET_HDR_LEN + ACK_HDR_LEN * 2 + PUSH_HDR_LEN + 1, 0);
        uploader.output_packet(&mut wtr).unwrap();

        //           0  1  2  3
        // to_ack
        // swnd     [ ]
        // to_send  [[8, 7]]
        assert!(uploader.to_ack_queue.is_empty());
        assert_eq!(uploader.swnd.size(), 1);

        assert_eq!(
            wtr.data(),
            vec![
                0, 99, // rwnd
                0, 0, 0, 88, // nack
                // ack
                0, 0, 0, 0, // seq
                1, // cmd: Ack
                // ack
                0, 0, 0, 1, // seq
                1, // cmd: Ack
                // push
                0, 0, 0, 0, // seq
                0, // cmd: Push
                0, 0, 0, 1, // len
                9, // body
            ]
        );

        thread::sleep(uploader.rto());

        let mut wtr = OwnedBufWtr::new(PACKET_HDR_LEN + PUSH_HDR_LEN + 1 + PUSH_HDR_LEN + 2, 0);
        uploader.output_packet(&mut wtr).unwrap();

        //           0  1  2  3
        // to_ack
        // swnd     [    ]
        // to_send  []
        assert!(uploader.to_send_queue.is_empty());
        assert_eq!(uploader.swnd.size(), 2);

        assert_eq!(
            wtr.data(),
            vec![
                0, 99, // rwnd
                0, 0, 0, 88, // nack
                // push
                0, 0, 0, 0, // seq
                0, // cmd: Push
                0, 0, 0, 1, // len
                9, // body
                // push
                0, 0, 0, 1, // seq
                0, // cmd: Push
                0, 0, 0, 2, // len
                8, 7 // body
            ]
        );
    }
}
