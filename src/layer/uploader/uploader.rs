use super::{
    super::{IObserver, SetUploadState},
    frag_bundler::FragBundler,
    SendingPush,
};
use crate::{
    protocol::{
        frag::{Body, Frag, FragBuilder, FragCommand, ACK_HDR_LEN, PUSH_HDR_LEN},
        packet::{Packet, PacketBuilder},
        packet_hdr::{PacketHeaderBuilder, PACKET_HDR_LEN},
    },
    utils::{
        buf::{self, BufPasta, BufSlicerQue},
        FastRetransmissionWnd, Seq32, Swnd,
    },
};
use keyed_priority_queue::KeyedPriorityQueue;
use std::{
    cmp,
    collections::VecDeque,
    sync::{Arc, Weak},
    time::{self, Duration, Instant},
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
    swnd: Swnd<Seq32, SendingPush>,
    to_ack_queue: VecDeque<Seq32>,
    last_sent_heap: KeyedPriorityQueue<Seq32, cmp::Reverse<Instant>>,

    // modified by setters
    local_rwnd_size: usize,
    local_next_seq_to_receive: Seq32,
    fast_retransmission_wnd: FastRetransmissionWnd<Seq32>,

    // stat
    stat: LocalStat,

    // const
    ratio_rto_to_one_rtt: f64,
    mtu: usize,

    // unit tests
    disable_rto: bool,

    // observer
    on_send_available: Option<Weak<dyn IObserver + Send + Sync + 'static>>,
}

pub struct UploaderBuilder {
    pub local_recv_buf_len: usize,
    pub nack_duplicate_threshold_to_activate_fast_retransmit: usize,
    pub ratio_rto_to_one_rtt: f64,
    pub mtu: usize,
    pub to_send_queue_len_cap: usize,
    pub swnd_size_cap: usize,
}

impl UploaderBuilder {
    #[must_use]
    pub fn build(self) -> Result<Uploader, BuildError> {
        if !(PACKET_HDR_LEN + ACK_HDR_LEN <= self.mtu)
            || !(PACKET_HDR_LEN + PUSH_HDR_LEN + 1 <= self.mtu)
        {
            return Err(BuildError::MtuTooSmall);
        }
        let this = Uploader {
            to_send_queue: BufSlicerQue::new(self.to_send_queue_len_cap),
            swnd: Swnd::new(self.swnd_size_cap),
            to_ack_queue: VecDeque::new(),
            local_rwnd_size: self.local_recv_buf_len,
            local_next_seq_to_receive: Seq32::from_u32(0),
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
            mtu: self.mtu,
            disable_rto: false,
            on_send_available: None,
            last_sent_heap: KeyedPriorityQueue::new(),
        };
        this.check_rep();
        Ok(this)
    }

    #[must_use]
    pub fn default() -> UploaderBuilder {
        let builder = Self {
            local_recv_buf_len: u16::MAX as usize,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            mtu: 1300,
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

#[derive(Debug)]
pub enum BuildError {
    MtuTooSmall,
}

pub struct SendError<T>(pub T);

impl Uploader {
    #[inline]
    fn check_rep(&self) {
        assert!(self.local_rwnd_size <= u16::MAX as usize);
    }

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

    pub fn write(&mut self, slice: buf::BufSlice) -> Result<(), SendError<buf::BufSlice>> {
        let result = match self.to_send_queue.push_back(slice) {
            Ok(_) => Ok(()),
            Err(e) => Err(SendError(e.0)),
        };
        result
    }

    #[must_use]
    pub fn emit(&mut self, now: &Instant) -> Vec<Packet> {
        let is_then_full = self.to_send_queue.is_full();
        let packets = self.emit_packets(self.mtu, now).unwrap();

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
        packets
    }

    fn emit_packets(
        &mut self,
        packet_space: usize,
        now: &Instant,
    ) -> Result<Vec<Packet>, OutputError> {
        if !(PACKET_HDR_LEN + ACK_HDR_LEN <= packet_space) {
            self.check_rep();
            return Err(OutputError::BufferTooSmall);
        }
        if !(PACKET_HDR_LEN + PUSH_HDR_LEN + 1 <= packet_space) {
            self.check_rep();
            return Err(OutputError::BufferTooSmall);
        }

        let bundles = self.emit_frags(packet_space - PACKET_HDR_LEN, now);
        let mut packets = Vec::new();

        for frags in bundles {
            // packet header
            let hdr = PacketHeaderBuilder {
                rwnd: self.local_rwnd_size as u16,
                nack: self.local_next_seq_to_receive,
            }
            .build()
            .unwrap();
            let packet = PacketBuilder { hdr, frags }.build().unwrap();
            packets.push(packet);
        }
        self.check_rep();
        Ok(packets)
    }

    #[inline]
    #[must_use]
    fn emit_frags(&mut self, space: usize, now: &Instant) -> Vec<Vec<Frag>> {
        let mut bundler = FragBundler::new(space);

        // piggyback ack
        loop {
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
            bundler.pack(frag).unwrap();
            self.stat.acks += 1;
        }

        // retransmission
        // write pushes from sending
        if !self.fast_retransmission_wnd.is_empty() {
            for (&seq, push) in self.swnd.range_mut(
                self.fast_retransmission_wnd.start(),
                self.fast_retransmission_wnd.end(),
            ) {
                {
                    // add push to collection
                    let frag = FragBuilder {
                        seq,
                        cmd: FragCommand::Push {
                            body: Body::Pasta(Arc::clone(push.body())),
                        },
                    }
                    .build()
                    .unwrap();
                    bundler.pack(frag).unwrap();
                    push.to_retransmit(*now); // test case: `test_rto_once`
                    self.last_sent_heap
                        .set_priority(&seq, cmp::Reverse(push.last_sent()))
                        .unwrap();
                }
                self.fast_retransmission_wnd.retransmitted(seq);
                self.stat.fast_retransmissions += 1;
                self.stat.retransmissions += 1;
                self.stat.pushes += 1;
            }
        }
        // min heap for rto
        let rto = self.rto();
        if !self.disable_rto {
            for _ in 0..self.last_sent_heap.len() {
                if let Some((&seq, last_sent)) = self.last_sent_heap.peek() {
                    let last_sent = last_sent.0;
                    if now.duration_since(last_sent) < rto {
                        break;
                    }
                    // write
                    if let Some(push) = self.swnd.value_mut(&seq) {
                        {
                            // add push to collection
                            let frag = FragBuilder {
                                seq,
                                cmd: FragCommand::Push {
                                    body: Body::Pasta(Arc::clone(push.body())),
                                },
                            }
                            .build()
                            .unwrap();
                            bundler.pack(frag).unwrap();
                            push.to_retransmit(*now);
                            self.last_sent_heap
                                .set_priority(&seq, cmp::Reverse(push.last_sent()))
                                .unwrap();
                        }
                        self.stat.rto_hits += 1;
                        self.stat.retransmissions += 1;
                        self.stat.pushes += 1;
                    } else {
                        self.last_sent_heap.pop().unwrap();
                    }
                } else {
                    break;
                }
            }
        }

        // move data from to_send queue to sending queue and output those data
        while !self.to_send_queue.is_empty() && !self.swnd.is_full() {
            // get as many bytes from to_send_queue to body
            let frag_body_limit = match PUSH_HDR_LEN + 1 <= bundler.loading_space() {
                true => bundler.loading_space() - PUSH_HDR_LEN,
                false => space - PUSH_HDR_LEN, // TODO: test when all body limit is used
            };
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

            let push = SendingPush::new(Arc::new(body), *now);

            // write the frag, including its hdr and body, to output buffer
            let seq = self.swnd.end();
            let frag = FragBuilder {
                seq,
                cmd: FragCommand::Push {
                    body: Body::Pasta(Arc::clone(push.body())),
                },
            }
            .build()
            .unwrap();
            bundler.pack(frag).unwrap();

            // register seq to the rto lookup
            self.last_sent_heap
                .push(seq, cmp::Reverse(push.last_sent()));

            // register the body to swnd
            self.swnd.push_back(push);

            self.stat.pushes += 1;
        }

        self.check_rep();
        return bundler.into_bundles();
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

    #[must_use]
    pub fn mtu(&self) -> usize {
        self.mtu
    }

    #[inline]
    fn set_remote_rwnd_size(&mut self, wnd: u16) {
        self.swnd.set_remote_rwnd_size(wnd as usize);
        self.check_rep();
    }

    #[inline]
    fn set_local_next_seq_to_receive(&mut self, local_next_seq_to_receive: Seq32) {
        self.local_next_seq_to_receive = local_next_seq_to_receive;
        self.check_rep();
    }

    #[inline]
    fn add_remote_seq_to_ack(&mut self, remote_seq_to_ack: Seq32) {
        self.to_ack_queue.push_back(remote_seq_to_ack);
        self.check_rep();
    }

    #[inline]
    fn set_acked_local_seq(&mut self, acked_local_seq: Seq32, now: &Instant) {
        // remove the selected sequence
        if let Some(frag) = self.swnd.remove(&acked_local_seq) {
            if !frag.is_retransmitted() {
                // set smooth RTT
                let frag_rtt = frag.since_last_sent(now);
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
    fn remove_sending_before(&mut self, remote_nack: Seq32) {
        self.swnd.remove_before(remote_nack);
        self.check_rep();
    }

    #[inline]
    fn set_local_rwnd_size(&mut self, local_rwnd_size: usize) {
        self.local_rwnd_size = local_rwnd_size;
        self.check_rep();
    }

    #[inline]
    pub fn set_state(&mut self, delta: SetUploadState, now: &Instant) -> Result<(), SetStateError> {
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
            self.set_acked_local_seq(acked_local_seq, now);
            max_acked_local_seq = Some(match max_acked_local_seq {
                Some(x) => Seq32::max(x, acked_local_seq),
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
        self.check_rep();
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
    pub next_seq_to_send: Seq32,
}

#[cfg(test)]
mod tests {
    use crate::{
        layer::{uploader::UploaderBuilder, SetUploadState},
        protocol::{
            frag::{Body, FragCommand, ACK_HDR_LEN, PUSH_HDR_LEN},
            packet_hdr::PACKET_HDR_LEN,
        },
        utils::{
            buf::{BufSlice, BufWtr, OwnedBufWtr},
            Seq32,
        },
    };
    use std::time::Instant;

    const MTU: usize = 512;

    #[test]
    fn test_empty() {
        let now = Instant::now();
        let mut uploader = UploaderBuilder::default().build().unwrap();
        let buf = OwnedBufWtr::new(MTU / 2, 0);
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 0);
    }

    #[test]
    fn test_few_1() {
        let now = Instant::now();
        let mut uploader = UploaderBuilder::default().build().unwrap();
        uploader.disable_rto = true;
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin = vec![0, 1, 2];
        buf.append(&origin).unwrap();
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        assert!(!uploader.to_send_queue.is_empty());
        let packets = uploader.emit(&now);
        {
            assert!(uploader.to_send_queue.is_empty());
            let mut body = OwnedBufWtr::new(origin.len(), 0);
            match packets[0].frags()[0].cmd() {
                FragCommand::Push { body: x } => match x {
                    Body::Slice(_) => panic!(),
                    Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                },
                FragCommand::Ack => panic!(),
            }
            assert_eq!(body.data(), origin);
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 0);
    }

    #[test]
    fn test_few_2() {
        let now = Instant::now();
        let mut builder = UploaderBuilder::default();
        builder.mtu = MTU;
        let mut uploader = builder.build().unwrap();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![3, 4];
        buf.append(&origin2).unwrap();
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        let packets = uploader.emit(&now);
        {
            assert!(uploader.to_send_queue.is_empty());
            let mut body = OwnedBufWtr::new(origin1.len() + origin2.len(), 0);
            match packets[0].frags()[0].cmd() {
                FragCommand::Push { body: x } => match x {
                    Body::Slice(_) => panic!(),
                    Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                },
                FragCommand::Ack => panic!(),
            }
            assert_eq!(body.data()[..origin1.len()], origin1);
            assert_eq!(body.data()[origin1.len()..], origin2);
        }
        assert_eq!(uploader.swnd.end().to_u32(), 1);
        assert_eq!(uploader.emit(&now).len(), 0);
    }

    #[test]
    fn test_few_many() {
        let now = Instant::now();
        let mut builder = UploaderBuilder::default();
        builder.mtu = MTU;
        let mut uploader = builder.build().unwrap();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin1 = vec![0, 1, 2];
        buf.append(&origin1).unwrap();
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin2 = vec![3; MTU];
        buf.append(&origin2).unwrap();
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        let packets = uploader.emit(&now);
        {
            assert_eq!(packets.len(), 1);
            let mut body = OwnedBufWtr::new(MTU, 0);
            match packets[0].frags()[0].cmd() {
                FragCommand::Push { body: x } => match x {
                    Body::Slice(_) => panic!(),
                    Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                },
                FragCommand::Ack => panic!(),
            }
            assert_eq!(body.data()[..origin1.len()], origin1);
            assert_eq!(
                body.data()[origin1.len()..],
                origin2[..MTU - PACKET_HDR_LEN - PUSH_HDR_LEN - origin1.len()]
            );
        }
        assert_eq!(uploader.swnd.end().to_u32(), 1);
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 0);
        assert_eq!(uploader.swnd.end().to_u32(), 1);

        uploader.set_remote_rwnd_size(10);

        let packets = uploader.emit(&now);
        {
            assert_eq!(packets.len(), 1);
            let mut body = OwnedBufWtr::new(MTU, 0);
            match packets[0].frags()[0].cmd() {
                FragCommand::Push { body: x } => match x {
                    Body::Slice(_) => panic!(),
                    Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                },
                FragCommand::Ack => panic!(),
            }
            assert_eq!(
                body.data(),
                &origin2[MTU - PACKET_HDR_LEN - PUSH_HDR_LEN - origin1.len()..]
            );
        }
        assert_eq!(uploader.swnd.end().to_u32(), 2);
        assert_eq!(uploader.emit(&now).len(), 0);
    }

    #[test]
    fn test_many_few() {
        let now = Instant::now();
        let mut builder = UploaderBuilder::default();
        builder.mtu = MTU;
        let mut uploader = builder.build().unwrap();
        let mut buf = OwnedBufWtr::new(MTU, 0);
        let origin1 = vec![3; MTU];
        buf.append(&origin1).unwrap();
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        let mut buf = OwnedBufWtr::new(MTU / 2, 0);
        let origin2 = vec![0, 1, 2];
        buf.append(&origin2).unwrap();
        let slice = BufSlice::from_wtr(buf);
        uploader.write(slice).map_err(|_| ()).unwrap();
        let packets = uploader.emit(&now);
        // packet: _hdr hdr mtu-_hdr-hdr
        // origin:          1[0..mtu-_hdr-hdr]
        {
            assert_eq!(packets.len(), 1);
            let mut body = OwnedBufWtr::new(MTU, 0);
            match packets[0].frags()[0].cmd() {
                FragCommand::Push { body: x } => match x {
                    Body::Slice(_) => panic!(),
                    Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                },
                FragCommand::Ack => panic!(),
            }
            assert_eq!(body.data(), &origin1[..MTU - PACKET_HDR_LEN - PUSH_HDR_LEN]);
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 0);

        uploader.set_remote_rwnd_size(10);

        let packets = uploader.emit(&now);
        // packet: _hdr hdr _hdr+hdr             3
        // origin:          1[mtu-_hdr-hdr..mtu] 2[0..3]
        {
            assert_eq!(packets.len(), 1);
            let mut body = OwnedBufWtr::new(MTU, 0);
            match packets[0].frags()[0].cmd() {
                FragCommand::Push { body: x } => match x {
                    Body::Slice(_) => panic!(),
                    Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                },
                FragCommand::Ack => panic!(),
            }
            assert_eq!(
                body.data()[..PACKET_HDR_LEN + PUSH_HDR_LEN],
                origin1[MTU - PACKET_HDR_LEN - PUSH_HDR_LEN..]
            );
            assert_eq!(body.data()[PACKET_HDR_LEN + PUSH_HDR_LEN..], origin2);
        }
        assert_eq!(uploader.emit(&now).len(), 0);
    }

    #[test]
    fn test_ack1() {
        let now = Instant::now();
        let mut builder = UploaderBuilder::default();
        builder.mtu = MTU;
        let mut uploader = builder.build().unwrap();
        uploader.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let mut buf = OwnedBufWtr::new(MTU / 2, 0);
            buf.append(&origin1).unwrap();
            let slice = BufSlice::from_wtr(buf);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let origin2 = vec![3, 4];
        {
            let mut buf = OwnedBufWtr::new(MTU / 2, 0);
            buf.append(&origin2).unwrap();
            let slice = BufSlice::from_wtr(buf);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let _ = uploader.emit(&now);

        assert_eq!(uploader.swnd.end().to_u32(), 1);
        assert_eq!(uploader.swnd.size(), 1);

        uploader.set_acked_local_seq(Seq32::from_u32(0), &now);

        assert_eq!(uploader.swnd.size(), 0);
    }

    #[test]
    fn test_rto_once() {
        let mut now = Instant::now();
        let mut builder = UploaderBuilder::default();
        builder.mtu = MTU;
        let mut uploader = builder.build().unwrap();
        uploader.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        now += uploader.rto();

        let packets = uploader.emit(&now);
        // last_seen of the fragment is refreshed
        assert_eq!(packets.len(), 1);

        let packets = uploader.emit(&now);
        // the fragment doesn't timeout
        assert_eq!(packets.len(), 0);
    }

    #[test]
    fn test_fast_retransmit1() {
        let now = Instant::now();
        let dup = 1;
        let mut uploader = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();
        uploader.disable_rto = true;
        uploader.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        // nack is 0 by default. When receiving another same nack, the fast retransmission gets activated since now the dup count becomes 1

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq32::from_u32(0),
            local_next_seq_to_receive: Seq32::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq32::from_u32(1)],
            local_rwnd_size: 1,
        };
        uploader.set_state(state, &now).unwrap();

        let packets = uploader.emit(&now);

        assert_eq!(packets.len(), 1);
    }

    #[test]
    fn test_fast_retransmit_no() {
        let now = Instant::now();
        let dup = 0;
        let mut uploader = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();
        uploader.disable_rto = true;
        uploader.set_remote_rwnd_size(2);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq32::from_u32(1),
            local_next_seq_to_receive: Seq32::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq32::from_u32(0)],
            local_rwnd_size: 1,
        };
        uploader.set_state(state, &now).unwrap();

        // remote wants seq(1)
        // since no out-of-order acks from the remote, we don't retransmit any seq

        // 0   1
        // ack nack

        let packets = uploader.emit(&now);

        assert_eq!(packets.len(), 0);
    }

    #[test]
    fn test_fast_retransmit2() {
        let now = Instant::now();
        let dup = 0;
        let mut uploader = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();
        uploader.disable_rto = true;
        uploader.set_remote_rwnd_size(99);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let origin3 = vec![4];
        {
            let slice = BufSlice::from_bytes(origin3);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq32::from_u32(1),
            local_next_seq_to_receive: Seq32::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq32::from_u32(2)],
            local_rwnd_size: 1,
        };
        uploader.set_state(state, &now).unwrap();

        // remote acked seq(2) but still wants seq(1)
        // clearly, seq(2) is an out-of-order ack, everything before it should be retransmitted if DUP meets

        // 0  1    2
        //    nack ack

        // seq(0) is implicitly acked by nack(1)

        // dup count for nack(1): 0

        let packets = uploader.emit(&now);

        assert_eq!(packets.len(), 1);
    }

    #[test]
    fn test_fast_retransmit3() {
        let now = Instant::now();
        let dup = 1;
        let mut uploader = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: dup,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();
        uploader.disable_rto = true;
        uploader.set_remote_rwnd_size(99);

        let origin1 = vec![0, 1, 2];
        {
            let slice = BufSlice::from_bytes(origin1);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let origin2 = vec![3];
        {
            let slice = BufSlice::from_bytes(origin2);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let origin3 = vec![4];
        {
            let slice = BufSlice::from_bytes(origin3);
            uploader.write(slice).map_err(|_| ()).unwrap();
        }
        let packets = uploader.emit(&now);
        assert_eq!(packets.len(), 1);

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq32::from_u32(1),
            local_next_seq_to_receive: Seq32::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq32::from_u32(2)],
            local_rwnd_size: 1,
        };
        uploader.set_state(state, &now).unwrap();

        // remote acked seq(2) but still wants seq(1)
        // clearly, seq(2) is an out-of-order ack, everything before it should be retransmitted if DUP meets

        // 0  1    2
        //    nack ack

        // seq(0) is implicitly acked by nack(1)

        // dup count for nack(1): 0

        let packets = uploader.emit(&now);

        assert_eq!(packets.len(), 0);

        let state = SetUploadState {
            remote_rwnd_size: 99,
            remote_nack: Seq32::from_u32(1),
            local_next_seq_to_receive: Seq32::from_u32(0),
            remote_seqs_to_ack: vec![],
            acked_local_seqs: vec![Seq32::from_u32(2)],
            local_rwnd_size: 1,
        };
        uploader.set_state(state, &now).unwrap();

        // dup count for nack(1): 1

        let packets = uploader.emit(&now);

        assert_eq!(packets.len(), 1);
    }

    #[test]
    fn test_multiple_frags() {
        let now = Instant::now();
        let mut uploader = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: PACKET_HDR_LEN + ACK_HDR_LEN * 2 + PUSH_HDR_LEN + 1,
        }
        .build()
        .unwrap();
        // uploader.disable_rto = true;

        //           0  1  2  3
        // to_ack
        // swnd    ][
        // to_send  []

        uploader
            .write(BufSlice::from_bytes(vec![9, 8, 7]))
            .map_err(|_| ())
            .unwrap();

        //           0  1  2  3
        // to_ack
        // swnd    ][
        // to_send  [[9, 8, 7]]
        assert!(!uploader.to_send_queue.is_empty());
        assert!(uploader.swnd.is_empty());

        uploader
            .set_state(
                SetUploadState {
                    remote_rwnd_size: 99,
                    remote_nack: Seq32::from_u32(2),
                    local_next_seq_to_receive: Seq32::from_u32(88),
                    remote_seqs_to_ack: vec![Seq32::from_u32(0), Seq32::from_u32(1)],
                    acked_local_seqs: Vec::new(),
                    local_rwnd_size: 99,
                },
                &now,
            )
            .unwrap();

        //           0  1  2  3
        // to_ack    x  x
        // swnd    ][
        // to_send  [[9, 8, 7]]
        assert_eq!(uploader.to_ack_queue.len(), 2);

        let packets = uploader.emit(&now);

        //           0  1  2  3
        // to_ack
        // swnd     [ ]
        // to_send  [[8, 7]]

        //           0  1  2  3
        // to_ack
        // swnd     [    ]
        // to_send  []
        assert!(uploader.to_ack_queue.is_empty());
        assert_eq!(uploader.swnd.size(), 2);

        {
            assert_eq!(packets.len(), 2);
            {
                assert_eq!(packets[0].hdr().rwnd(), 99);
                assert_eq!(packets[0].hdr().nack(), Seq32::from_u32(88));
                assert_eq!(packets[0].frags().len(), 3);
                assert_eq!(packets[0].frags()[0].seq().to_u32(), 0);
                match packets[0].frags()[0].cmd() {
                    FragCommand::Push { body: _ } => panic!(),
                    FragCommand::Ack => (),
                }
                assert_eq!(packets[0].frags()[1].seq().to_u32(), 1);
                match packets[0].frags()[1].cmd() {
                    FragCommand::Push { body: _ } => panic!(),
                    FragCommand::Ack => (),
                }
                assert_eq!(packets[0].frags()[2].seq().to_u32(), 0);
                let mut body = OwnedBufWtr::new(1, 0);
                match packets[0].frags()[2].cmd() {
                    FragCommand::Push { body: x } => match x {
                        Body::Slice(_) => panic!(),
                        Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                    },
                    FragCommand::Ack => panic!(),
                }
                assert_eq!(body.data(), vec![9]);
            }
            {
                assert_eq!(packets[1].frags().len(), 1);
                let mut body = OwnedBufWtr::new(2, 0);
                match packets[1].frags()[0].cmd() {
                    FragCommand::Push { body: x } => match x {
                        Body::Slice(_) => panic!(),
                        Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                    },
                    FragCommand::Ack => panic!(),
                }
                assert_eq!(body.data(), vec![8, 7]);
            }
        }

        // thread::sleep(uploader.rto());

        // let mut wtr = OwnedBufWtr::new(PACKET_HDR_LEN + PUSH_HDR_LEN + 1 + PUSH_HDR_LEN + 2, 0);
        // uploader.output_packets(&mut wtr).unwrap();

        // //           0  1  2  3
        // // to_ack
        // // swnd     [    ]
        // // to_send  []
        // assert!(uploader.to_send_queue.is_empty());
        // assert_eq!(uploader.swnd.size(), 2);

        // assert_eq!(
        //     wtr.data(),
        //     vec![
        //         0, 99, // rwnd
        //         0, 0, 0, 88, // nack
        //         // push
        //         0, 0, 0, 0, // seq
        //         0, // cmd: Push
        //         0, 0, 0, 1, // len
        //         9, // body
        //         // push
        //         0, 0, 0, 1, // seq
        //         0, // cmd: Push
        //         0, 0, 0, 2, // len
        //         8, 7 // body
        //     ]
        // );
    }

    #[test]
    fn test_body_pasta() {
        let now = Instant::now();
        let mut uploader = UploaderBuilder {
            local_recv_buf_len: 0,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: PACKET_HDR_LEN + PUSH_HDR_LEN + 6,
        }
        .build()
        .unwrap();
        uploader.disable_rto = true;

        uploader
            .write(BufSlice::from_bytes(vec![0, 1]))
            .map_err(|_| ())
            .unwrap();
        uploader
            .write(BufSlice::from_bytes(vec![2]))
            .map_err(|_| ())
            .unwrap();
        uploader
            .write(BufSlice::from_bytes(vec![3, 4, 5]))
            .map_err(|_| ())
            .unwrap();

        // to_send  [[0, 1], [2], [3, 4, 5]]

        let packets = uploader.emit(&now);
        {
            assert_eq!(packets.len(), 1);
            let packet = &packets[0];
            assert_eq!(packet.frags().len(), 1);
            let mut body = OwnedBufWtr::new(6, 0);
            match packet.frags()[0].cmd() {
                FragCommand::Push { body: x } => match x {
                    Body::Slice(_) => panic!(),
                    Body::Pasta(x) => x.append_to(&mut body).unwrap(),
                },
                FragCommand::Ack => panic!(),
            };
            assert_eq!(body.data(), vec![0, 1, 2, 3, 4, 5]);
        }
    }
}
