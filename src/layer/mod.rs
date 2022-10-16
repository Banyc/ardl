mod downloader;
mod observer;
mod uploader;

use crate::utils::Seq32;
pub use downloader::*;
pub use observer::*;
pub use uploader::*;

pub struct Builder {
    pub local_recv_buf_len: usize,
    pub nack_duplicate_threshold_to_activate_fast_retransmit: usize,
    pub ratio_rto_to_one_rtt: f64,
    pub to_send_queue_len_cap: usize,
    pub swnd_size_cap: usize,
    pub mtu: usize,
}

impl Builder {
    pub fn build(self) -> Result<(Uploader, Downloader), BuildError> {
        let uploader = UploaderBuilder {
            local_recv_buf_len: self.local_recv_buf_len,
            nack_duplicate_threshold_to_activate_fast_retransmit: self
                .nack_duplicate_threshold_to_activate_fast_retransmit,
            ratio_rto_to_one_rtt: self.ratio_rto_to_one_rtt,
            to_send_queue_len_cap: self.to_send_queue_len_cap,
            swnd_size_cap: self.swnd_size_cap,
            mtu: self.mtu,
        }
        .build()
        .map_err(|e| BuildError::Uploader(e))?;
        let downloader = DownloaderBuilder {
            recv_buf_len: self.local_recv_buf_len,
        }
        .build()
        .map_err(|e| BuildError::Downloader(e))?;
        Ok((uploader, downloader))
    }

    pub fn default() -> Self {
        Builder {
            local_recv_buf_len: 1024,
            nack_duplicate_threshold_to_activate_fast_retransmit: 1024 * 1 / 2,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: 1024,
            swnd_size_cap: 1024,
            mtu: 1300,
        }
    }
}

#[derive(Debug)]
pub enum BuildError {
    Downloader(downloader::BuildError),
    Uploader(uploader::BuildError),
}

pub struct SetUploadState {
    pub remote_rwnd_size: u16,
    pub remote_nack: Seq32,
    pub local_next_seq_to_receive: Seq32,
    pub remote_seqs_to_ack: Vec<Seq32>,
    pub acked_local_seqs: Vec<Seq32>,
    pub local_rwnd_size: usize,
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crate::utils::buf::{BufSlice, BufWtr, OwnedBufWtr};

    use super::Builder;

    const MTU: usize = 1024;

    #[test]
    fn test_few_1() {
        let now = Instant::now();
        let (mut upload1, mut download1) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();
        let (mut upload2, mut download2) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();

        // push: 1 -> 2
        {
            let buf = vec![0, 1, 2];
            let slice = BufSlice::from_bytes(buf);
            upload1.write(slice).map_err(|_| ()).unwrap();

            let mut inflight = OwnedBufWtr::new(1024, 0);
            let packets = upload1.emit(&now);

            assert_eq!(packets.len(), 1);

            packets[0].append_to(&mut inflight).unwrap();

            assert_eq!(
                inflight.data(),
                vec![
                    0, 2, // rwnd
                    0, 0, 0, 0, // nack
                    0, 0, 0, 0, // seq
                    0, // cmd (Push)
                    0, 0, 0, 3, // len
                    0, 1, 2 // data
                ]
            );

            let inflight = BufSlice::from_wtr(inflight);
            let upload2_changes = download2.write(inflight).unwrap();
            upload2.set_state(upload2_changes, &now).unwrap();

            let recv2 = download2.emit().unwrap();
            assert_eq!(recv2.data(), vec![0, 1, 2]);
        }
        // ack: 1 <- 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let packets = upload2.emit(&now);

            assert_eq!(packets.len(), 1);

            packets[0].append_to(&mut inflight).unwrap();

            //                               rwnd] [     nack] [      seq] [cmd
            assert_eq!(inflight.data(), vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

            let inflight = BufSlice::from_wtr(inflight);
            let upload1_changes = download1.write(inflight).unwrap();
            upload1.set_state(upload1_changes, &now).unwrap();
        }
    }

    #[test]
    fn test_rto() {
        let mut now = Instant::now();
        let (mut upload1, mut _download1) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();
        let (mut upload2, mut download2) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_len_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
            mtu: MTU,
        }
        .build()
        .unwrap();

        // push: 1 -> 2
        {
            let buf = vec![0, 1, 2];
            let slice = BufSlice::from_bytes(buf);
            upload1.write(slice).map_err(|_| ()).unwrap();

            let mut inflight = OwnedBufWtr::new(1024, 0);
            let packets = upload1.emit(&now);

            assert_eq!(packets.len(), 1);

            packets[0].append_to(&mut inflight).unwrap();

            assert_eq!(
                inflight.data(),
                vec![
                    0, 2, // rwnd
                    0, 0, 0, 0, // nack
                    0, 0, 0, 0, // seq
                    0, // cmd (Push)
                    0, 0, 0, 3, // len
                    0, 1, 2 // data
                ]
            );

            let inflight = BufSlice::from_wtr(inflight);
            let upload2_changes = download2.write(inflight).unwrap();
            upload2.set_state(upload2_changes, &now).unwrap();

            let recv2 = download2.emit().unwrap();
            assert_eq!(recv2.data(), vec![0, 1, 2]);
        }
        // ack: 1 <- 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let packets = upload2.emit(&now);

            assert_eq!(packets.len(), 1);

            packets[0].append_to(&mut inflight).unwrap();

            //                               rwnd] [     nack] [      seq] [cmd
            assert_eq!(inflight.data(), vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

            // dropped
        }
        now += upload1.rto();
        // retransmit: 1 -> 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let packets = upload1.emit(&now);

            assert_eq!(packets.len(), 1);

            packets[0].append_to(&mut inflight).unwrap();

            assert_eq!(
                inflight.data(),
                vec![
                    0, 2, // rwnd
                    0, 0, 0, 0, // nack
                    0, 0, 0, 0, // seq
                    0, // cmd (Push)
                    0, 0, 0, 3, // len
                    0, 1, 2 // data
                ]
            );
        }
    }
}
