use crate::utils::Seq;

mod downloader;
pub use downloader::*;
mod uploader;
pub use uploader::*;

pub struct Builder {
    pub local_recv_buf_len: usize,
    pub nack_duplicate_threshold_to_activate_fast_retransmit: usize,
    pub ratio_rto_to_one_rtt: f64,
    pub to_send_queue_byte_cap: usize,
    pub swnd_size_cap: usize,
}

impl Builder {
    pub fn build(self) -> (Uploader, Downloader) {
        let upload = UploaderBuilder {
            local_recv_buf_len: self.local_recv_buf_len,
            nack_duplicate_threshold_to_activate_fast_retransmit: self
                .nack_duplicate_threshold_to_activate_fast_retransmit,
            ratio_rto_to_one_rtt: self.ratio_rto_to_one_rtt,
            to_send_queue_byte_cap: self.to_send_queue_byte_cap,
            swnd_size_cap: self.swnd_size_cap,
        }
        .build();
        let download = DownloaderBuilder {
            recv_buf_len: self.local_recv_buf_len,
        }
        .build();
        (upload, download)
    }
}

pub struct SetUploadState {
    pub remote_rwnd_size: u16,
    pub remote_nack: Seq,
    pub local_next_seq_to_receive: Seq,
    pub remote_seqs_to_ack: Vec<Seq>,
    pub acked_local_seqs: Vec<Seq>,
    pub local_rwnd_size: usize,
}

#[cfg(test)]
mod tests {
    use std::thread;

    use crate::utils::buf::{BufRdr, BufWtr, OwnedBufWtr};

    use super::Builder;

    #[test]
    fn test_few_1() {
        let (mut upload1, mut download1) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_byte_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();
        let (mut upload2, mut download2) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_byte_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();

        // push: 1 -> 2
        {
            let buf = vec![0, 1, 2];
            let rdr = BufRdr::from_bytes(buf);
            upload1.to_send(rdr).map_err(|_| ()).unwrap();

            let mut inflight = OwnedBufWtr::new(1024, 0);
            let is_written = upload1.append_packet_to_and_if_written(&mut inflight);
            assert!(is_written);

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

            let inflight = BufRdr::from_wtr(inflight);
            let upload2_changes = download2.input_packet(inflight).unwrap();
            upload2.set_state(upload2_changes).unwrap();

            let recv2 = download2.recv().unwrap();
            assert_eq!(recv2.data(), vec![0, 1, 2]);
        }
        // ack: 1 <- 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let is_written = upload2.append_packet_to_and_if_written(&mut inflight);
            assert!(is_written);

            //                               rwnd] [     nack] [      seq] [cmd
            assert_eq!(inflight.data(), vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

            let inflight = BufRdr::from_wtr(inflight);
            let upload1_changes = download1.input_packet(inflight).unwrap();
            upload1.set_state(upload1_changes).unwrap();
        }
    }

    #[test]
    fn test_rto() {
        let (mut upload1, mut _download1) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_byte_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();
        let (mut upload2, mut download2) = Builder {
            local_recv_buf_len: 2,
            nack_duplicate_threshold_to_activate_fast_retransmit: 0,
            ratio_rto_to_one_rtt: 1.5,
            to_send_queue_byte_cap: usize::MAX,
            swnd_size_cap: usize::MAX,
        }
        .build();

        // push: 1 -> 2
        {
            let buf = vec![0, 1, 2];
            let rdr = BufRdr::from_bytes(buf);
            upload1.to_send(rdr).map_err(|_| ()).unwrap();

            let mut inflight = OwnedBufWtr::new(1024, 0);
            let is_written = upload1.append_packet_to_and_if_written(&mut inflight);
            assert!(is_written);

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

            let inflight = BufRdr::from_wtr(inflight);
            let upload2_changes = download2.input_packet(inflight).unwrap();
            upload2.set_state(upload2_changes).unwrap();

            let recv2 = download2.recv().unwrap();
            assert_eq!(recv2.data(), vec![0, 1, 2]);
        }
        // ack: 1 <- 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let is_written = upload2.append_packet_to_and_if_written(&mut inflight);
            assert!(is_written);

            //                               rwnd] [     nack] [      seq] [cmd
            assert_eq!(inflight.data(), vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

            // dropped
        }
        thread::sleep(upload1.rto());
        // retransmit: 1 -> 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let is_written = upload1.append_packet_to_and_if_written(&mut inflight);
            assert!(is_written);

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
