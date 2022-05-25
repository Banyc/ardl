use std::collections::{BTreeMap, VecDeque};

use crate::{
    protocols::yatcp::{
        frag_hdr::{FragCommand, FragHeader},
        packet_hdr::PacketHeader,
    },
    utils::{self, BufFrag, Seq},
};

use super::SetUploadState;

pub struct YatcpDownload {
    received_queue: VecDeque<BufFrag>,
    receiving_queue: BTreeMap<Seq, BufFrag>,
    local_next_seq_to_receive: Seq,
    max_local_receiving_queue_len: usize, // inclusive
    stat: LocalStat,
}

pub struct YatcpDownloadBuilder {
    pub max_local_receiving_queue_len: usize,
}

impl YatcpDownloadBuilder {
    pub fn build(self) -> YatcpDownload {
        let this = YatcpDownload {
            received_queue: VecDeque::new(),
            receiving_queue: BTreeMap::new(),
            local_next_seq_to_receive: Seq::from_u32(0),
            max_local_receiving_queue_len: self.max_local_receiving_queue_len,
            stat: LocalStat {
                out_of_windows: 0,
                out_of_orders: 0,
                decoding_errors: 0,
            },
        };
        this.check_rep();
        this
    }
}

#[derive(Debug)]
pub enum Error {
    Decoding,
}

impl YatcpDownload {
    #[inline]
    fn check_rep(&self) {
        assert!(self.max_local_receiving_queue_len > 0);
        assert!(self.max_local_receiving_queue_len <= u16::MAX as usize);
        assert!(self.receiving_queue.len() <= self.max_local_receiving_queue_len);
        for (&seq, _) in &self.receiving_queue {
            assert!(self.local_next_seq_to_receive < seq);
            break;
        }
    }

    pub fn stat(&self) -> Stat {
        Stat {
            out_of_windows: self.stat.out_of_windows,
            out_of_orders: self.stat.out_of_orders,
            decoding_errors: self.stat.decoding_errors,
            next_seq_to_receive: self.local_next_seq_to_receive,
        }
    }

    pub fn recv(&mut self) -> Option<BufFrag> {
        let received = self.received_queue.pop_front();
        self.check_rep();
        received
    }

    pub fn recv_max(&mut self, max_len: usize) -> Option<BufFrag> {
        let received = match self.received_queue.pop_front() {
            Some(x) => {
                if x.len() > max_len {
                    let slice_front = x.slice(0..max_len).unwrap();
                    let slice_end = x.slice(max_len..x.len()).unwrap();
                    self.received_queue.push_front(slice_end);
                    Some(slice_front)
                } else {
                    Some(x)
                }
            }
            None => None,
        };
        self.check_rep();
        received
    }

    pub fn input(&mut self, mut rdr: utils::BufRdr) -> Result<SetUploadState, Error> {
        let partial_state_changes = self.handle_packet(&mut rdr)?;
        let state_changes = SetUploadState {
            remote_rwnd: partial_state_changes.remote_rwnd,
            remote_nack: partial_state_changes.remote_nack,
            local_next_seq_to_receive: self.local_next_seq_to_receive,
            remote_seqs_to_ack: partial_state_changes.frags.remote_seqs_to_ack,
            acked_local_seqs: partial_state_changes.frags.acked_local_seqs,
            local_receiving_queue_free_len: self.max_local_receiving_queue_len
                - self.receiving_queue.len(),
        };
        Ok(state_changes)
    }

    fn handle_packet(
        &mut self,
        rdr: &mut utils::BufRdr,
    ) -> Result<HandlePacketStateChanges, Error> {
        let mut cursor = rdr.get_peek_cursor();
        let hdr = match PacketHeader::from_bytes(&mut cursor) {
            Ok(x) => x,
            Err(_) => {
                self.stat.decoding_errors += 1;
                return Err(Error::Decoding);
            }
        };
        let read_len = cursor.position();
        drop(cursor);
        rdr.skip(read_len as usize).unwrap();

        let partial_state_changes = self.handle_frags(rdr);
        let state_changes = HandlePacketStateChanges {
            frags: partial_state_changes,
            remote_rwnd: hdr.wnd(),
            remote_nack: hdr.nack(),
        };
        Ok(state_changes)
    }

    fn handle_frags(&mut self, rdr: &mut utils::BufRdr) -> HandleFragsStateChanges {
        let mut remote_seqs_to_ack = Vec::new();
        let mut acked_local_seqs = Vec::new();
        loop {
            if rdr.is_empty() {
                break;
            }

            let mut cursor = rdr.get_peek_cursor();
            let hdr = match FragHeader::from_bytes(&mut cursor) {
                Ok(x) => x,
                // TODO: review
                // a whole fragment is ignorable => best efforts
                Err(_) => {
                    self.stat.decoding_errors += 1;
                    break;
                }
            };
            let read_len = cursor.position();
            drop(cursor);
            rdr.skip(read_len as usize).unwrap();

            match hdr.cmd() {
                FragCommand::Push { len } => {
                    if *len == 0 {
                        // TODO: review
                        // if `cmd::push`, `len` is not allowed to be `0`
                        self.stat.decoding_errors += 1;
                        break;
                    }
                    let body = match rdr.try_slice(*len as usize) {
                        Some(x) => x,
                        // no transactions are happening => no need to compensate
                        None => {
                            self.stat.decoding_errors += 1;
                            break;
                        }
                    };
                    // if out of rwnd
                    if !(hdr.seq()
                        < self
                            .local_next_seq_to_receive
                            .add_u32(self.max_local_receiving_queue_len as u32)
                        && self.local_next_seq_to_receive <= hdr.seq())
                    {
                        self.stat.out_of_windows += 1;
                        // drop the fragment
                    } else {
                        // schedule uploader to ack this seq
                        remote_seqs_to_ack.push(hdr.seq());

                        if hdr.seq() == self.local_next_seq_to_receive {
                            // skip inserting this consecutive fragment to rwnd
                            // hot path
                            self.received_queue.push_back(body);
                            self.local_next_seq_to_receive.increment();
                        } else {
                            // insert this fragment to rwnd
                            self.receiving_queue.insert(hdr.seq(), body);
                        }

                        // pop consecutive fragments from the rwnd to the ready queue
                        while let Some(frag) =
                            self.receiving_queue.remove(&self.local_next_seq_to_receive)
                        {
                            self.received_queue.push_back(frag);
                            self.local_next_seq_to_receive.increment();
                        }
                    }
                }
                FragCommand::Ack => {
                    acked_local_seqs.push(hdr.seq());
                }
            }
        }

        HandleFragsStateChanges {
            remote_seqs_to_ack,
            acked_local_seqs,
        }
    }
}

struct HandleFragsStateChanges {
    remote_seqs_to_ack: Vec<Seq>,
    acked_local_seqs: Vec<Seq>,
}

struct HandlePacketStateChanges {
    frags: HandleFragsStateChanges,
    remote_rwnd: u16,
    remote_nack: Seq,
}

struct LocalStat {
    out_of_windows: u64,
    out_of_orders: u64,
    decoding_errors: u64,
}

#[derive(Debug)]
pub struct Stat {
    pub out_of_windows: u64,
    pub out_of_orders: u64,
    pub decoding_errors: u64,
    pub next_seq_to_receive: Seq,
}

#[cfg(test)]
mod tests {
    use crate::{
        protocols::yatcp::{
            frag_hdr::{FragCommand, FragHeaderBuilder},
            packet_hdr::PacketHeaderBuilder,
        },
        utils::{BufRdr, Seq},
    };

    use super::YatcpDownloadBuilder;

    #[test]
    fn test_empty() {
        let mut download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: 3,
        }
        .build();

        let origin1 = vec![];
        let rdr = BufRdr::from_bytes(origin1);
        let changes = download.input(rdr);
        assert!(changes.is_err());
    }

    #[test]
    fn test_few_1() {
        let mut download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: 3,
        }
        .build();

        let mut buf = Vec::new();
        let packet_hdr = PacketHeaderBuilder {
            rwnd: 2,
            nack: Seq::from_u32(0),
        }
        .build()
        .unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: Seq::from_u32(0),
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 1);
        assert_eq!(changes.local_receiving_queue_free_len, 3);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd, 2);
        let tmp: Vec<Seq> = vec![0].iter().map(|&x| Seq::from_u32(x)).collect();
        assert_eq!(changes.remote_seqs_to_ack, tmp);
        assert_eq!(changes.acked_local_seqs, vec![]);
        assert_eq!(download.recv().unwrap().data(), vec![4; 11]);
    }

    #[test]
    fn test_out_of_order() {
        let mut download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: 3,
        }
        .build();

        let mut buf = Vec::new();
        let packet_hdr = PacketHeaderBuilder {
            rwnd: 2,
            nack: Seq::from_u32(0),
        }
        .build()
        .unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: Seq::from_u32(1),
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(changes.local_receiving_queue_free_len, 2);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd, 2);
        let tmp: Vec<Seq> = vec![1].iter().map(|&x| Seq::from_u32(x)).collect();
        assert_eq!(changes.remote_seqs_to_ack, tmp);
        assert_eq!(changes.acked_local_seqs, vec![]);
        assert!(download.recv().is_none());
    }

    #[test]
    fn test_out_of_window1() {
        let mut download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: 3,
        }
        .build();

        let mut buf = Vec::new();
        let packet_hdr = PacketHeaderBuilder {
            rwnd: 2,
            nack: Seq::from_u32(0),
        }
        .build()
        .unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: Seq::from_u32(99),
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(changes.local_receiving_queue_free_len, 3);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd, 2);
        assert_eq!(changes.remote_seqs_to_ack, vec![]);
        assert_eq!(changes.acked_local_seqs, vec![]);
        assert!(download.recv().is_none());
    }

    #[test]
    fn test_ack() {
        let mut download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: 3,
        }
        .build();

        let mut buf = Vec::new();
        let packet_hdr = PacketHeaderBuilder {
            rwnd: 2,
            nack: Seq::from_u32(0),
        }
        .build()
        .unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let ack1 = FragHeaderBuilder {
            seq: Seq::from_u32(1),
            cmd: FragCommand::Ack,
        }
        .build()
        .unwrap();
        buf.append(&mut ack1.to_bytes());
        let ack2 = FragHeaderBuilder {
            seq: Seq::from_u32(3),
            cmd: FragCommand::Ack,
        }
        .build()
        .unwrap();
        buf.append(&mut ack2.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: Seq::from_u32(99),
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(changes.local_receiving_queue_free_len, 3);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd, 2);
        assert_eq!(changes.remote_seqs_to_ack, vec![]);
        let tmp: Vec<Seq> = vec![1, 3].iter().map(|&x| Seq::from_u32(x)).collect();
        assert_eq!(changes.acked_local_seqs, tmp);
        assert!(download.recv().is_none());
    }

    #[test]
    fn test_rwnd_proceeding() {
        let mut download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: 2,
        }
        .build();

        {
            let mut buf = Vec::new();
            let packet_hdr = PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq::from_u32(0),
            }
            .build()
            .unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr1 = FragHeaderBuilder {
                    seq: Seq::from_u32(1),
                    cmd: FragCommand::Push { len: 1 },
                }
                .build()
                .unwrap();
                let mut push_body1 = vec![1; 1];
                buf.append(&mut push_hdr1.to_bytes());
                buf.append(&mut push_body1);
            }
            {
                let push_hdr2 = FragHeaderBuilder {
                    seq: Seq::from_u32(2),
                    cmd: FragCommand::Push { len: 2 },
                }
                .build()
                .unwrap();
                let mut push_body2 = vec![2; 2];
                buf.append(&mut push_hdr2.to_bytes());
                buf.append(&mut push_body2);
            }

            let rdr = BufRdr::from_bytes(buf);
            let changes = download.input(rdr).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
            assert_eq!(changes.local_receiving_queue_free_len, 1);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd, 2);
            let tmp: Vec<Seq> = vec![1].iter().map(|&x| Seq::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert!(download.recv().is_none());
        }
        {
            let mut buf = Vec::new();
            let packet_hdr = PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq::from_u32(0),
            }
            .build()
            .unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr0 = FragHeaderBuilder {
                    seq: Seq::from_u32(0),
                    cmd: FragCommand::Push { len: 1 },
                }
                .build()
                .unwrap();
                let mut push_body0 = vec![0; 1];
                buf.append(&mut push_hdr0.to_bytes());
                buf.append(&mut push_body0);
            }
            {
                let push_hdr3 = FragHeaderBuilder {
                    seq: Seq::from_u32(3),
                    cmd: FragCommand::Push { len: 3 },
                }
                .build()
                .unwrap();
                let mut push_body3 = vec![3; 3];
                buf.append(&mut push_hdr3.to_bytes());
                buf.append(&mut push_body3);
            }

            let rdr = BufRdr::from_bytes(buf);
            let changes = download.input(rdr).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 2);
            assert_eq!(changes.local_receiving_queue_free_len, 1);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd, 2);
            let tmp: Vec<Seq> = vec![0, 3].iter().map(|&x| Seq::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv().unwrap().data(), vec![0; 1]);
            assert_eq!(download.recv().unwrap().data(), vec![1; 1]);
        }
        {
            let mut buf = Vec::new();
            let packet_hdr = PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq::from_u32(0),
            }
            .build()
            .unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr2 = FragHeaderBuilder {
                    seq: Seq::from_u32(2),
                    cmd: FragCommand::Push { len: 2 },
                }
                .build()
                .unwrap();
                let mut push_body2 = vec![2; 2];
                buf.append(&mut push_hdr2.to_bytes());
                buf.append(&mut push_body2);
            }

            let rdr = BufRdr::from_bytes(buf);
            let changes = download.input(rdr).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 4);
            assert_eq!(changes.local_receiving_queue_free_len, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd, 2);
            let tmp: Vec<Seq> = vec![2].iter().map(|&x| Seq::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv().unwrap().data(), vec![2; 2]);
            assert_eq!(download.recv().unwrap().data(), vec![3; 3]);
        }
        // test out of window2
        {
            let mut buf = Vec::new();
            let packet_hdr = PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq::from_u32(0),
            }
            .build()
            .unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr0 = FragHeaderBuilder {
                    seq: Seq::from_u32(0),
                    cmd: FragCommand::Push { len: 2 },
                }
                .build()
                .unwrap();
                let mut push_body0 = vec![2; 2];
                buf.append(&mut push_hdr0.to_bytes());
                buf.append(&mut push_body0);
            }

            let rdr = BufRdr::from_bytes(buf);
            let changes = download.input(rdr).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 4);
            assert_eq!(changes.local_receiving_queue_free_len, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd, 2);
            assert_eq!(changes.remote_seqs_to_ack, vec![]);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert!(download.recv().is_none());
        }
    }

    #[test]
    fn test_recv_max() {
        let mut download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: 3,
        }
        .build();

        let mut buf = Vec::new();
        {
            let packet_hdr = PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq::from_u32(0),
            }
            .build()
            .unwrap();
            buf.append(&mut packet_hdr.to_bytes());
        }
        {
            let push_hdr1 = FragHeaderBuilder {
                seq: Seq::from_u32(0),
                cmd: FragCommand::Push { len: 4 },
            }
            .build()
            .unwrap();
            let mut push_body1 = vec![0, 1, 2, 3];
            buf.append(&mut push_hdr1.to_bytes());
            buf.append(&mut push_body1);
        }
        {
            let rdr = BufRdr::from_bytes(buf);
            let changes = download.input(rdr).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 1);
            assert_eq!(changes.local_receiving_queue_free_len, 3);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd, 2);
            let tmp: Vec<Seq> = vec![0].iter().map(|&x| Seq::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv_max(1).unwrap().data(), vec![0]);
            assert_eq!(download.recv_max(2).unwrap().data(), vec![1, 2]);
            assert_eq!(download.recv_max(10).unwrap().data(), vec![3]);
        }
    }
}
