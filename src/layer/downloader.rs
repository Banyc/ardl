use super::SetUploadState;
use crate::{
    protocol::{
        frag::{Body, Frag, FragCommand},
        packet::Packet,
    },
    utils::{
        buf::{self, BufSlice},
        RecvBuf, Seq32, SeqLocationToRwnd,
    },
};

pub struct Downloader {
    recv_buf: RecvBuf<Seq32, BufSlice>,
    leftover: Option<BufSlice>,
    stat: LocalStat,
}

pub struct DownloaderBuilder {
    pub recv_buf_len: usize,
}

impl DownloaderBuilder {
    pub fn build(self) -> Result<Downloader, BuildError> {
        if !(self.recv_buf_len <= u16::MAX as usize) {
            return Err(BuildError::RecvBufTooLarge);
        }
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
        Ok(this)
    }
}

#[derive(Debug)]
pub enum BuildError {
    RecvBufTooLarge,
}

#[derive(Debug)]
pub enum Error {
    Decoding,
}

impl Downloader {
    #[inline]
    fn check_rep(&self) {
        assert!(self.recv_buf.rwnd_size() <= u16::MAX as usize);
    }

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
    pub fn input_packet(&mut self, mut slice: buf::BufSlice) -> Result<SetUploadState, Error> {
        let packet = Packet::from_slice(&mut slice).map_err(|_| {
            self.stat.decoding_errors += 1;
            self.check_rep();
            Error::Decoding
        })?;
        let packet_state = self.handle_packet(packet);
        let state = SetUploadState {
            remote_rwnd_size: packet_state.remote_rwnd,
            remote_nack: packet_state.remote_nack,
            local_next_seq_to_receive: self.recv_buf.next_seq_to_receive(),
            remote_seqs_to_ack: packet_state.frags.remote_seqs_to_ack,
            acked_local_seqs: packet_state.frags.acked_local_seqs,
            local_rwnd_size: self.recv_buf.rwnd_size(),
        };
        self.check_rep();
        Ok(state)
    }

    #[must_use]
    fn handle_packet(&mut self, packet: Packet) -> PacketState {
        let packet = packet.into_builder();
        let frags_state = self.handle_frags(packet.frags);
        let state = PacketState {
            frags: frags_state,
            remote_rwnd: packet.hdr.rwnd(),
            remote_nack: packet.hdr.nack(),
        };
        self.stat.packets += 1;
        self.check_rep();
        state
    }

    #[must_use]
    fn handle_frags(&mut self, frags: Vec<Frag>) -> FragsState {
        let mut remote_seqs_to_ack = Vec::new();
        let mut acked_local_seqs = Vec::new();
        for frag in frags {
            let frag = frag.into_builder();
            match frag.cmd {
                FragCommand::Push { body } => {
                    let body = match body {
                        Body::Slice(x) => x,
                        Body::Pasta(_) => panic!(),
                    };
                    // if out of rwnd
                    let location = self.recv_buf.insert(frag.seq, body);
                    match location {
                        SeqLocationToRwnd::InRecvWindow => {
                            // schedule uploader to ack this seq
                            remote_seqs_to_ack.push(frag.seq);

                            self.stat.out_of_orders += 1;
                        }
                        SeqLocationToRwnd::AtRecvWindowStart => {
                            // schedule uploader to ack this seq
                            remote_seqs_to_ack.push(frag.seq);
                        }
                        SeqLocationToRwnd::TooLate => {
                            // schedule uploader to ack this seq
                            remote_seqs_to_ack.push(frag.seq);

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
                    acked_local_seqs.push(frag.seq);
                    self.stat.acks += 1;
                }
            }
        }
        self.check_rep();
        FragsState {
            remote_seqs_to_ack,
            acked_local_seqs,
        }
    }
}

struct FragsState {
    remote_seqs_to_ack: Vec<Seq32>,
    acked_local_seqs: Vec<Seq32>,
}

struct PacketState {
    frags: FragsState,
    remote_rwnd: u16,
    remote_nack: Seq32,
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
    pub next_seq_to_receive: Seq32,
    pub packets: u64,
    pub acks: u64,
    pub pushes: u64,
}

#[cfg(test)]
mod tests {
    use crate::{
        protocol::{
            frag::{Body, FragBuilder, FragCommand},
            packet::PacketBuilder,
            packet_hdr::PacketHeaderBuilder,
        },
        utils::{
            buf::{BufSlice, OwnedBufWtr},
            Seq32,
        },
    };

    use super::DownloaderBuilder;

    #[test]
    fn test_empty() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build().unwrap();

        let origin1 = vec![];
        let slice = BufSlice::from_bytes(origin1);
        let changes = download.input_packet(slice);
        assert!(changes.is_err());
    }

    #[test]
    fn test_few_1() {
        let mut downloader = DownloaderBuilder { recv_buf_len: 3 }.build().unwrap();

        let packet = PacketBuilder {
            hdr: PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq32::from_u32(0),
            }
            .build()
            .unwrap(),
            frags: vec![FragBuilder {
                seq: Seq32::from_u32(0),
                cmd: FragCommand::Push {
                    body: Body::Slice(BufSlice::from_bytes(vec![4; 11])),
                },
            }
            .build()
            .unwrap()],
        }
        .build()
        .unwrap();

        // [packet_header] [push_hdr seq(0)] [11]

        let mut wtr = OwnedBufWtr::new(1024, 0);
        packet.append_to(&mut wtr).unwrap();
        let slice = BufSlice::from_wtr(wtr);
        let state = downloader.input_packet(slice).unwrap();
        assert_eq!(state.local_next_seq_to_receive.to_u32(), 1);
        assert_eq!(state.local_rwnd_size, 2);
        assert_eq!(state.remote_nack.to_u32(), 0);
        assert_eq!(state.remote_rwnd_size, 2);
        let tmp: Vec<Seq32> = vec![0].iter().map(|&x| Seq32::from_u32(x)).collect();
        assert_eq!(state.remote_seqs_to_ack, tmp);
        assert_eq!(state.acked_local_seqs, vec![]);
        assert_eq!(downloader.recv().unwrap().data(), vec![4; 11]);
    }

    #[test]
    fn test_out_of_order() {
        let mut downloader = DownloaderBuilder { recv_buf_len: 3 }.build().unwrap();

        let packet = PacketBuilder {
            hdr: PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq32::from_u32(0),
            }
            .build()
            .unwrap(),
            frags: vec![FragBuilder {
                seq: Seq32::from_u32(1),
                cmd: FragCommand::Push {
                    body: Body::Slice(BufSlice::from_bytes(vec![4; 11])),
                },
            }
            .build()
            .unwrap()],
        }
        .build()
        .unwrap();

        // [packet_header] [push_hdr seq(1)] [11]

        let mut wtr = OwnedBufWtr::new(1024, 0);
        packet.append_to(&mut wtr).unwrap();
        let slice = BufSlice::from_wtr(wtr);
        let state = downloader.input_packet(slice).unwrap();
        assert_eq!(state.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(state.local_rwnd_size, 3);
        assert_eq!(state.remote_nack.to_u32(), 0);
        assert_eq!(state.remote_rwnd_size, 2);
        let tmp: Vec<Seq32> = vec![1].iter().map(|&x| Seq32::from_u32(x)).collect();
        assert_eq!(state.remote_seqs_to_ack, tmp);
        assert_eq!(state.acked_local_seqs, vec![]);
        assert!(downloader.recv().is_none());
    }

    #[test]
    fn test_out_of_window1() {
        let mut downloader = DownloaderBuilder { recv_buf_len: 3 }.build().unwrap();

        let packet = PacketBuilder {
            hdr: PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq32::from_u32(0),
            }
            .build()
            .unwrap(),
            frags: vec![FragBuilder {
                seq: Seq32::from_u32(99),
                cmd: FragCommand::Push {
                    body: Body::Slice(BufSlice::from_bytes(vec![4; 11])),
                },
            }
            .build()
            .unwrap()],
        }
        .build()
        .unwrap();

        let mut wtr = OwnedBufWtr::new(1024, 0);
        packet.append_to(&mut wtr).unwrap();
        let slice = BufSlice::from_wtr(wtr);
        let state = downloader.input_packet(slice).unwrap();
        assert_eq!(state.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(state.local_rwnd_size, 3);
        assert_eq!(state.remote_nack.to_u32(), 0);
        assert_eq!(state.remote_rwnd_size, 2);
        assert_eq!(state.remote_seqs_to_ack, vec![]);
        assert_eq!(state.acked_local_seqs, vec![]);
        assert!(downloader.recv().is_none());
    }

    #[test]
    fn test_ack() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build().unwrap();

        let packet = PacketBuilder {
            hdr: PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq32::from_u32(0),
            }
            .build()
            .unwrap(),
            frags: vec![
                FragBuilder {
                    seq: Seq32::from_u32(1),
                    cmd: FragCommand::Ack,
                }
                .build()
                .unwrap(),
                FragBuilder {
                    seq: Seq32::from_u32(3),
                    cmd: FragCommand::Ack,
                }
                .build()
                .unwrap(),
                FragBuilder {
                    seq: Seq32::from_u32(99),
                    cmd: FragCommand::Push {
                        body: Body::Slice(BufSlice::from_bytes(vec![4; 11])),
                    },
                }
                .build()
                .unwrap(),
            ],
        }
        .build()
        .unwrap();

        let mut wtr = OwnedBufWtr::new(1024, 0);
        packet.append_to(&mut wtr).unwrap();
        let slice = BufSlice::from_wtr(wtr);
        let state = download.input_packet(slice).unwrap();
        assert_eq!(state.local_next_seq_to_receive.to_u32(), 0);
        assert_eq!(state.local_rwnd_size, 3);
        assert_eq!(state.remote_nack.to_u32(), 0);
        assert_eq!(state.remote_rwnd_size, 2);
        assert_eq!(state.remote_seqs_to_ack, vec![]);
        let tmp: Vec<Seq32> = vec![1, 3].iter().map(|&x| Seq32::from_u32(x)).collect();
        assert_eq!(state.acked_local_seqs, tmp);
        assert!(download.recv().is_none());
    }

    #[test]
    fn test_rwnd_proceeding() {
        let mut downloader = DownloaderBuilder { recv_buf_len: 2 }.build().unwrap();

        {
            let packet = PacketBuilder {
                hdr: PacketHeaderBuilder {
                    rwnd: 2,
                    nack: Seq32::from_u32(0),
                }
                .build()
                .unwrap(),
                frags: vec![
                    FragBuilder {
                        seq: Seq32::from_u32(1),
                        cmd: FragCommand::Push {
                            body: Body::Slice(BufSlice::from_bytes(vec![1; 1])),
                        },
                    }
                    .build()
                    .unwrap(),
                    FragBuilder {
                        seq: Seq32::from_u32(2),
                        cmd: FragCommand::Push {
                            body: Body::Slice(BufSlice::from_bytes(vec![2; 2])),
                        },
                    }
                    .build()
                    .unwrap(),
                ],
            }
            .build()
            .unwrap();

            // [packet_header] [push_hdr seq(1)] [1] [push_hdr seq(2)] [2]

            let mut wtr = OwnedBufWtr::new(1024, 0);
            packet.append_to(&mut wtr).unwrap();
            let slice = BufSlice::from_wtr(wtr);
            let changes = downloader.input_packet(slice).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 0);
            assert_eq!(changes.local_rwnd_size, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            let tmp: Vec<Seq32> = vec![1].iter().map(|&x| Seq32::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert!(downloader.recv().is_none());
        }
        {
            let packet = PacketBuilder {
                hdr: PacketHeaderBuilder {
                    rwnd: 2,
                    nack: Seq32::from_u32(0),
                }
                .build()
                .unwrap(),
                frags: vec![
                    FragBuilder {
                        seq: Seq32::from_u32(0),
                        cmd: FragCommand::Push {
                            body: Body::Slice(BufSlice::from_bytes(vec![0; 1])),
                        },
                    }
                    .build()
                    .unwrap(),
                    FragBuilder {
                        seq: Seq32::from_u32(3),
                        cmd: FragCommand::Push {
                            body: Body::Slice(BufSlice::from_bytes(vec![3; 3])),
                        },
                    }
                    .build()
                    .unwrap(),
                ],
            }
            .build()
            .unwrap();

            // [packet_header] [push_hdr seq(0)] [1] [push_hdr seq(3)] [3]

            let mut wtr = OwnedBufWtr::new(1024, 0);
            packet.append_to(&mut wtr).unwrap();
            let slice = BufSlice::from_wtr(wtr);
            let state = downloader.input_packet(slice).unwrap();
            assert_eq!(state.local_next_seq_to_receive.to_u32(), 2);
            assert_eq!(state.local_rwnd_size, 0);
            assert_eq!(state.remote_nack.to_u32(), 0);
            assert_eq!(state.remote_rwnd_size, 2);
            let tmp: Vec<Seq32> = vec![0].iter().map(|&x| Seq32::from_u32(x)).collect();
            assert_eq!(state.remote_seqs_to_ack, tmp);
            assert_eq!(state.acked_local_seqs, vec![]);
            assert_eq!(downloader.recv().unwrap().data(), vec![0; 1]);
            assert_eq!(downloader.recv().unwrap().data(), vec![1; 1]);
        }
        {
            let packet = PacketBuilder {
                hdr: PacketHeaderBuilder {
                    rwnd: 2,
                    nack: Seq32::from_u32(0),
                }
                .build()
                .unwrap(),
                frags: vec![FragBuilder {
                    seq: Seq32::from_u32(2),
                    cmd: FragCommand::Push {
                        body: Body::Slice(BufSlice::from_bytes(vec![2; 2])),
                    },
                }
                .build()
                .unwrap()],
            }
            .build()
            .unwrap();

            // [packet_header] [push_hdr seq(2)] [2]

            let mut wtr = OwnedBufWtr::new(1024, 0);
            packet.append_to(&mut wtr).unwrap();
            let slice = BufSlice::from_wtr(wtr);
            let changes = downloader.input_packet(slice).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 3);
            assert_eq!(changes.local_rwnd_size, 1);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            let tmp: Vec<Seq32> = vec![2].iter().map(|&x| Seq32::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(downloader.recv().unwrap().data(), vec![2; 2]);
            assert!(downloader.recv().is_none());
        }
        // test out of window2
        {
            let packet = PacketBuilder {
                hdr: PacketHeaderBuilder {
                    rwnd: 2,
                    nack: Seq32::from_u32(0),
                }
                .build()
                .unwrap(),
                frags: vec![FragBuilder {
                    seq: Seq32::from_u32(0),
                    cmd: FragCommand::Push {
                        body: Body::Slice(BufSlice::from_bytes(vec![2; 2])),
                    },
                }
                .build()
                .unwrap()],
            }
            .build()
            .unwrap();

            // [packet_header] [push_hdr seq(0)] [2]

            let mut wtr = OwnedBufWtr::new(1024, 0);
            packet.append_to(&mut wtr).unwrap();
            let slice = BufSlice::from_wtr(wtr);
            let changes = downloader.input_packet(slice).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 3);
            assert_eq!(changes.local_rwnd_size, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            assert_eq!(changes.remote_seqs_to_ack, vec![Seq32::from_u32(0)]);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert!(downloader.recv().is_none());
        }
    }

    #[test]
    fn test_recv_max() {
        let mut download = DownloaderBuilder { recv_buf_len: 3 }.build().unwrap();

        let packet = PacketBuilder {
            hdr: PacketHeaderBuilder {
                rwnd: 2,
                nack: Seq32::from_u32(0),
            }
            .build()
            .unwrap(),
            frags: vec![FragBuilder {
                seq: Seq32::from_u32(0),
                cmd: FragCommand::Push {
                    body: Body::Slice(BufSlice::from_bytes(vec![0, 1, 2, 3])),
                },
            }
            .build()
            .unwrap()],
        }
        .build()
        .unwrap();

        // [packet_header] [push_hdr seq(0)] [4]

        {
            let mut wtr = OwnedBufWtr::new(1024, 0);
            packet.append_to(&mut wtr).unwrap();
            let slice = BufSlice::from_wtr(wtr);
            let changes = download.input_packet(slice).unwrap();
            assert_eq!(changes.local_next_seq_to_receive.to_u32(), 1);
            assert_eq!(changes.local_rwnd_size, 2);
            assert_eq!(changes.remote_nack.to_u32(), 0);
            assert_eq!(changes.remote_rwnd_size, 2);
            let tmp: Vec<Seq32> = vec![0].iter().map(|&x| Seq32::from_u32(x)).collect();
            assert_eq!(changes.remote_seqs_to_ack, tmp);
            assert_eq!(changes.acked_local_seqs, vec![]);
            assert_eq!(download.recv_max(1).unwrap().data(), vec![0]);
            assert_eq!(download.recv_max(2).unwrap().data(), vec![1, 2]);
            assert_eq!(download.recv_max(10).unwrap().data(), vec![3]);
        }
    }

    #[test]
    fn test_large_rwnd() {
        let recv_buf_len = (u16::MAX as usize) + 1;
        let result = DownloaderBuilder { recv_buf_len }.build();
        match result {
            Ok(_) => panic!(),
            Err(_) => (),
        }
    }
}
