use std::collections::{BTreeMap, VecDeque};

use crate::{
    protocols::yatcp::{
        frag_hdr::{FragCommand, FragHeader},
        packet_hdr::PacketHeader,
    },
    utils::{self, BufFrag},
};

pub struct YatcpDownload {
    received_queue: VecDeque<BufFrag>,
    receiving_queue: BTreeMap<u32, BufFrag>,
    local_next_seq_to_receive: u32,
    max_local_receiving_queue_len: usize, // inclusive
}

pub struct YatcpDownloadBuilder {
    pub max_local_receiving_queue_len: usize,
}

impl YatcpDownloadBuilder {
    pub fn build(self) -> YatcpDownload {
        let this = YatcpDownload {
            received_queue: VecDeque::new(),
            receiving_queue: BTreeMap::new(),
            local_next_seq_to_receive: 0,
            max_local_receiving_queue_len: self.max_local_receiving_queue_len,
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
        assert!(self.receiving_queue.len() <= self.max_local_receiving_queue_len);
        for (&seq, _) in &self.receiving_queue {
            assert!(self.local_next_seq_to_receive < seq);
            break;
        }
    }

    pub fn recv(&mut self) -> Option<BufFrag> {
        let received = self.received_queue.pop_front();
        self.check_rep();
        received
    }

    pub fn input(&mut self, mut rdr: utils::BufRdr) -> Result<UploadStateChanges, Error> {
        let partial_state_changes = self.handle_packet(&mut rdr)?;
        let state_changes = UploadStateChanges {
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
            Err(_) => return Err(Error::Decoding),
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
                Err(_) => break,
            };
            let read_len = cursor.position();
            drop(cursor);
            rdr.skip(read_len as usize).unwrap();

            match hdr.cmd() {
                FragCommand::Push { len } => {
                    let body = match rdr.try_slice(*len as usize) {
                        Some(x) => x,
                        None => todo!(),
                    };
                    // if out of rwnd
                    if !(hdr.seq()
                        < self.local_next_seq_to_receive
                            + (self.max_local_receiving_queue_len as u32)
                        && self.local_next_seq_to_receive <= hdr.seq())
                    {
                        // drop the fragment
                    } else {
                        // insert this fragment to rwnd
                        self.receiving_queue.insert(hdr.seq(), body);
                        remote_seqs_to_ack.push(hdr.seq());

                        while let Some(frag) =
                            self.receiving_queue.remove(&self.local_next_seq_to_receive)
                        {
                            self.received_queue.push_back(frag);
                            self.local_next_seq_to_receive += 1;
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

pub struct UploadStateChanges {
    pub remote_rwnd: u16,
    pub remote_nack: u32,
    pub local_next_seq_to_receive: u32,
    pub remote_seqs_to_ack: Vec<u32>,
    pub acked_local_seqs: Vec<u32>,
    pub local_receiving_queue_free_len: usize,
}

struct HandleFragsStateChanges {
    remote_seqs_to_ack: Vec<u32>,
    acked_local_seqs: Vec<u32>,
}

struct HandlePacketStateChanges {
    frags: HandleFragsStateChanges,
    remote_rwnd: u16,
    remote_nack: u32,
}

#[cfg(test)]
mod tests {
    use crate::{
        protocols::yatcp::{
            frag_hdr::{FragCommand, FragHeaderBuilder},
            packet_hdr::PacketHeaderBuilder,
        },
        utils::BufRdr,
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
        let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: 0,
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive, 1);
        assert_eq!(changes.local_receiving_queue_free_len, 3);
        assert_eq!(changes.remote_nack, 0);
        assert_eq!(changes.remote_rwnd, 2);
        assert_eq!(changes.remote_seqs_to_ack, vec![0]);
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
        let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: 1,
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive, 0);
        assert_eq!(changes.local_receiving_queue_free_len, 2);
        assert_eq!(changes.remote_nack, 0);
        assert_eq!(changes.remote_rwnd, 2);
        assert_eq!(changes.remote_seqs_to_ack, vec![1]);
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
        let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: 99,
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive, 0);
        assert_eq!(changes.local_receiving_queue_free_len, 3);
        assert_eq!(changes.remote_nack, 0);
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
        let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
        buf.append(&mut packet_hdr.to_bytes());
        let ack1 = FragHeaderBuilder {
            seq: 1,
            cmd: FragCommand::Ack,
        }
        .build()
        .unwrap();
        buf.append(&mut ack1.to_bytes());
        let ack2 = FragHeaderBuilder {
            seq: 3,
            cmd: FragCommand::Ack,
        }
        .build()
        .unwrap();
        buf.append(&mut ack2.to_bytes());
        let push_hdr1 = FragHeaderBuilder {
            seq: 99,
            cmd: FragCommand::Push { len: 11 },
        }
        .build()
        .unwrap();
        let mut push_body1 = vec![4; 11];
        buf.append(&mut push_hdr1.to_bytes());
        buf.append(&mut push_body1);

        let rdr = BufRdr::from_bytes(buf);
        let changes = download.input(rdr).unwrap();
        assert_eq!(changes.local_next_seq_to_receive, 0);
        assert_eq!(changes.local_receiving_queue_free_len, 3);
        assert_eq!(changes.remote_nack, 0);
        assert_eq!(changes.remote_rwnd, 2);
        assert_eq!(changes.remote_seqs_to_ack, vec![]);
        assert_eq!(changes.acked_local_seqs, vec![1, 3]);
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
            let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr1 = FragHeaderBuilder {
                    seq: 1,
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
                    seq: 2,
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
            assert_eq!(changes.local_next_seq_to_receive, 0);
            assert_eq!(changes.local_receiving_queue_free_len, 1);
            assert_eq!(changes.remote_nack, 0);
            assert_eq!(changes.remote_rwnd, 2);
            assert_eq!(changes.remote_seqs_to_ack, vec![1]);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert!(download.recv().is_none());
        }
        {
            let mut buf = Vec::new();
            let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr0 = FragHeaderBuilder {
                    seq: 0,
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
                    seq: 3,
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
            assert_eq!(changes.local_next_seq_to_receive, 2);
            assert_eq!(changes.local_receiving_queue_free_len, 1);
            assert_eq!(changes.remote_nack, 0);
            assert_eq!(changes.remote_rwnd, 2);
            assert_eq!(changes.remote_seqs_to_ack, vec![0, 3]);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv().unwrap().data(), vec![0; 1]);
            assert_eq!(download.recv().unwrap().data(), vec![1; 1]);
        }
        {
            let mut buf = Vec::new();
            let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr2 = FragHeaderBuilder {
                    seq: 2,
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
            assert_eq!(changes.local_next_seq_to_receive, 4);
            assert_eq!(changes.local_receiving_queue_free_len, 2);
            assert_eq!(changes.remote_nack, 0);
            assert_eq!(changes.remote_rwnd, 2);
            assert_eq!(changes.remote_seqs_to_ack, vec![2]);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv().unwrap().data(), vec![2; 2]);
            assert_eq!(download.recv().unwrap().data(), vec![3; 3]);
        }
        // test out of window2
        {
            let mut buf = Vec::new();
            let packet_hdr = PacketHeaderBuilder { rwnd: 2, nack: 0 }.build().unwrap();
            buf.append(&mut packet_hdr.to_bytes());
            {
                let push_hdr0 = FragHeaderBuilder {
                    seq: 0,
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
            assert_eq!(changes.local_next_seq_to_receive, 4);
            assert_eq!(changes.local_receiving_queue_free_len, 2);
            assert_eq!(changes.remote_nack, 0);
            assert_eq!(changes.remote_rwnd, 2);
            assert_eq!(changes.remote_seqs_to_ack, vec![]);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert!(download.recv().is_none());
        }
    }
}
