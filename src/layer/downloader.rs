use std::io::Cursor;

use crate::{
    protocol::{
        frag_hdr::{FragCommand, FragHeader},
        packet_hdr::PacketHeader,
    },
    utils::{
        buf::{self, BufSlice},
        RecvBuf, Seq, SeqLocationToRwnd,
    },
};

use super::SetUploadState;

pub struct Downloader {
    recv_buf: RecvBuf<BufSlice>,
    leftover: Option<BufSlice>,
    stat: LocalStat,
}

pub struct DownloaderBuilder {
    pub recv_buf_len: usize,
}

impl DownloaderBuilder {
    pub fn build(self) -> Downloader {
        let this = Downloader {
            recv_buf: RecvBuf::new(self.recv_buf_len),
            leftover: None,
            stat: LocalStat {
                early_pushes: 0,
                late_pushes: 0,
                out_of_orders: 0,
                decoding_errors: 0,
                packets: 0,
                acks: 0,
                pushes: 0,
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

impl Downloader {
    #[inline]
    fn check_rep(&self) {}

    #[must_use]
    pub fn stat(&self) -> Stat {
        Stat {
            early_pushes: self.stat.early_pushes,
            late_pushes: self.stat.late_pushes,
            out_of_orders: self.stat.out_of_orders,
            decoding_errors: self.stat.decoding_errors,
            next_seq_to_receive: self.recv_buf.next_seq_to_receive(),
            packets: self.stat.packets,
            pushes: self.stat.pushes,
            acks: self.stat.acks,
        }
    }

    #[must_use]
    pub fn recv(&mut self) -> Option<BufSlice> {
        let received = self.recv_buf.pop_front();
        self.check_rep();
        received
    }

    #[must_use]
    pub fn recv_max(&mut self, max_len: usize) -> Option<BufSlice> {
        let leftover = self.leftover.take();
        let slice = if let Some(slice) = leftover {
            slice
        } else {
            if let Some(slice) = self.recv_buf.pop_front() {
                slice
            } else {
                return None;
            }
        };

        let final_slice = if slice.len() > max_len {
            let (head, tail) = slice.split(max_len).unwrap();
            self.leftover = Some(tail);
            Some(head)
        } else {
            Some(slice)
        };

        self.check_rep();
        final_slice
    }

    #[must_use]
    pub fn input_packet(&mut self, mut rdr: buf::BufRdr) -> Result<SetUploadState, Error> {
        let partial_state_changes = self.handle_packet(&mut rdr)?;
        let state_changes = SetUploadState {
            remote_rwnd_size: partial_state_changes.remote_rwnd,
            remote_nack: partial_state_changes.remote_nack,
            local_next_seq_to_receive: self.recv_buf.next_seq_to_receive(),
            remote_seqs_to_ack: partial_state_changes.frags.remote_seqs_to_ack,
            acked_local_seqs: partial_state_changes.frags.acked_local_seqs,
            local_rwnd_size: self.recv_buf.rwnd_size(),
        };
        Ok(state_changes)
    }

    #[must_use]
    fn handle_packet(&mut self, rdr: &mut buf::BufRdr) -> Result<HandlePacketStateChanges, Error> {
        let mut cursor = Cursor::new(rdr.data());
        let hdr = match PacketHeader::from_bytes(&mut cursor) {
            Ok(x) => x,
            Err(_) => {
                self.stat.decoding_errors += 1;
                return Err(Error::Decoding);
            }
        };
        let read_len = cursor.position();
        drop(cursor);
        rdr.skip_precisely(read_len as usize).unwrap();

        let partial_state_changes = self.handle_frags(rdr);
        let state_changes = HandlePacketStateChanges {
            frags: partial_state_changes,
            remote_rwnd: hdr.wnd(),
            remote_nack: hdr.nack(),
        };
        self.stat.packets += 1;
        Ok(state_changes)
    }

    #[must_use]
    fn handle_frags(&mut self, rdr: &mut buf::BufRdr) -> HandleFragsStateChanges {
        let mut remote_seqs_to_ack = Vec::new();
        let mut acked_local_seqs = Vec::new();
        loop {
            if rdr.is_empty() {
                break;
            }

            let mut cursor = Cursor::new(rdr.data());
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
            rdr.skip_precisely(read_len as usize).unwrap();

            match hdr.cmd() {
                FragCommand::Push { len } => {
                    if *len == 0 {
                        // TODO: review
                        // if `cmd::push`, `len` is not allowed to be `0`
                        self.stat.decoding_errors += 1;
                        break;
                    }
                    let body = match rdr.take_precisely(*len as usize) {
                        Ok(x) => x,
                        // no transactions are happening => no need to compensate
                        Err(_) => {
                            self.stat.decoding_errors += 1;
                            break;
                        }
                    };
                    // if out of rwnd
                    let location = self.recv_buf.insert(hdr.seq(), body);
                    match location {
                        SeqLocationToRwnd::InRecvWindow => {
                            // schedule uploader to ack this seq
                            remote_seqs_to_ack.push(hdr.seq());

                            self.stat.out_of_orders += 1;
                        }
                        SeqLocationToRwnd::AtRecvWindowStart => {
                            // schedule uploader to ack this seq
                            remote_seqs_to_ack.push(hdr.seq());
                        }
                        SeqLocationToRwnd::TooLate => {
                            // schedule uploader to ack this seq
                            remote_seqs_to_ack.push(hdr.seq());

                            self.stat.late_pushes += 1;
                            // drop the fragment
                        }
                        SeqLocationToRwnd::TooEarly => {
                            self.stat.early_pushes += 1;
                            // drop the fragment
                        }
                    }
                    self.stat.pushes += 1;
                }
                FragCommand::Ack => {
                    acked_local_seqs.push(hdr.seq());
                    self.stat.acks += 1;
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
    late_pushes: u64,
    early_pushes: u64,
    out_of_orders: u64,
    decoding_errors: u64,
    packets: u64,
    acks: u64,
    pushes: u64,
}

#[derive(Debug, PartialEq)]
pub struct Stat {
    pub late_pushes: u64,
    pub early_pushes: u64,
    pub out_of_orders: u64,
    pub decoding_errors: u64,
    pub next_seq_to_receive: Seq,
    pub packets: u64,
    pub acks: u64,
    pub pushes: u64,
}

#[cfg(test)]
mod tests {
    use crate::{
        protocol::{
            frag_hdr::{FragCommand, FragHeaderBuilder},
            packet_hdr::PacketHeaderBuilder,
        },
        utils::{
            buf::{BufRdr, BufSlice},
            Seq,
        },
    };

    use super::DownloaderBuilder;

    #[test]
    fn test_empty() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build();

        let origin1 = vec![];
        let slice = BufSlice::from_bytes(origin1);
        let changes = download.input_packet(BufRdr::from_slice(slice));
        assert!(changes.is_err());
    }

    #[test]
    fn test_few_1() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build();

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

        // [packet_header] [push_hdr seq(0)] [11]

        let slice = BufSlice::from_bytes(buf);
        let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 1);
        assert_eq!(changes.local_rwnd_size, 2);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd_size, 2);
        let tmp: Vec<Seq> = vec![0].iter().map(|&x| Seq::from_u32(x)).collect();
        assert_eq!(changes.remote_seqs_to_ack, tmp);
        assert_eq!(changes.acked_local_seqs, vec![]);
        assert_eq!(download.recv().unwrap().data(), vec![4; 11]);
    }

    #[test]
    fn test_out_of_order() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build();

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

        // [packet_header] [push_hdr seq(1)] [11]

        let slice = BufSlice::from_bytes(buf);
        let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(changes.local_rwnd_size, 3);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd_size, 2);
        let tmp: Vec<Seq> = vec![1].iter().map(|&x| Seq::from_u32(x)).collect();
        assert_eq!(changes.remote_seqs_to_ack, tmp);
        assert_eq!(changes.acked_local_seqs, vec![]);
        assert!(download.recv().is_none());
    }

    #[test]
    fn test_out_of_window1() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build();

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

        let slice = BufSlice::from_bytes(buf);
        let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(changes.local_rwnd_size, 3);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd_size, 2);
        assert_eq!(changes.remote_seqs_to_ack, vec![]);
        assert_eq!(changes.acked_local_seqs, vec![]);
        assert!(download.recv().is_none());
    }

    #[test]
    fn test_ack() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build();

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

        let slice = BufSlice::from_bytes(buf);
        let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
        assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(changes.local_rwnd_size, 3);
        assert_eq!(changes.remote_nack.to_u32(), 0);
        assert_eq!(changes.remote_rwnd_size, 2);
        assert_eq!(changes.remote_seqs_to_ack, vec![]);
        let tmp: Vec<Seq> = vec![1, 3].iter().map(|&x| Seq::from_u32(x)).collect();
        assert_eq!(changes.acked_local_seqs, tmp);
        assert!(download.recv().is_none());
    }

    #[test]
    fn test_rwnd_proceeding() {
        let mut download = DownloaderBuilder { recv_buf_len: 2 }.build();

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

            // [packet_header] [push_hdr seq(1)] [1] [push_hdr seq(2)] [2]

            let slice = BufSlice::from_bytes(buf);
            let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
            assert_eq!(changes.local_rwnd_size, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
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

            // [packet_header] [push_hdr seq(0)] [1] [push_hdr seq(3)] [3]

            let slice = BufSlice::from_bytes(buf);
            let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 2);
            assert_eq!(changes.local_rwnd_size, 0);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            let tmp: Vec<Seq> = vec![0].iter().map(|&x| Seq::from_u32(x)).collect();
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

            // [packet_header] [push_hdr seq(2)] [2]

            let slice = BufSlice::from_bytes(buf);
            let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 3);
            assert_eq!(changes.local_rwnd_size, 1);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            let tmp: Vec<Seq> = vec![2].iter().map(|&x| Seq::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv().unwrap().data(), vec![2; 2]);
            assert!(download.recv().is_none());
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

            // [packet_header] [push_hdr seq(0)] [2]

            let slice = BufSlice::from_bytes(buf);
            let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 3);
            assert_eq!(changes.local_rwnd_size, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            assert_eq!(changes.remote_seqs_to_ack, vec![Seq::from_u32(0)]);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert!(download.recv().is_none());
        }
    }

    #[test]
    fn test_recv_max() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build();

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

        // [packet_header] [push_hdr seq(0)] [4]

        {
            let slice = BufSlice::from_bytes(buf);
            let changes = download.input_packet(BufRdr::from_slice(slice)).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 1);
            assert_eq!(changes.local_rwnd_size, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            let tmp: Vec<Seq> = vec![0].iter().map(|&x| Seq::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv_max(1).unwrap().data(), vec![0]);
            assert_eq!(download.recv_max(2).unwrap().data(), vec![1, 2]);
            assert_eq!(download.recv_max(10).unwrap().data(), vec![3]);
        }
    }
}
