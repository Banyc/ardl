use std::time;

use self::{
    yatcp_download::{YatcpDownload, YatcpDownloadBuilder},
    yatcp_upload::{YatcpUpload, YatcpUploadBuilder},
};

pub mod yatcp_download;
pub mod yatcp_upload;

pub struct YatcpBuilder {
    pub max_local_receiving_queue_len: usize,
    pub re_tx_timeout: time::Duration,
}

impl YatcpBuilder {
    pub fn build(self) -> (YatcpUpload, YatcpDownload) {
        let upload = YatcpUploadBuilder {
            local_receiving_queue_len: self.max_local_receiving_queue_len,
            re_tx_timeout: self.re_tx_timeout,
        }
        .build();
        let download = YatcpDownloadBuilder {
            max_local_receiving_queue_len: self.max_local_receiving_queue_len,
        }
        .build();
        (upload, download)
    }
}

pub struct SetUploadStates {
    pub remote_rwnd: u16,
    pub remote_nack: u32,
    pub local_next_seq_to_receive: u32,
    pub remote_seqs_to_ack: Vec<u32>,
    pub acked_local_seqs: Vec<u32>,
    pub local_receiving_queue_free_len: usize,
}

#[cfg(test)]
mod tests {
    use std::time;

    use crate::utils::{BufRdr, BufWtr, OwnedBufWtr};

    use super::YatcpBuilder;

    #[test]
    fn test_few_1() {
        let (mut upload1, mut download1) = YatcpBuilder {
            max_local_receiving_queue_len: 2,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();
        let (mut upload2, mut download2) = YatcpBuilder {
            max_local_receiving_queue_len: 2,
            re_tx_timeout: time::Duration::from_secs(99),
        }
        .build();

        // push: 1 -> 2
        {
            let buf = vec![0, 1, 2];
            let rdr = BufRdr::from_bytes(buf);
            upload1.to_send(rdr);

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
            let upload2_changes = download2.input(inflight).unwrap();
            upload2.set_states(upload2_changes);

            let recv2 = download2.recv().unwrap();
            assert_eq!(recv2.data(), vec![0, 1, 2]);
        }
        // ack: 1 <- 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let is_written = upload2.append_packet_to_and_if_written(&mut inflight);
            assert!(is_written);

            //                               rwnd] [     nack] [      seq] [cmd
            assert_eq!(inflight.data(), vec![0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

            let inflight = BufRdr::from_wtr(inflight);
            let upload1_changes = download1.input(inflight).unwrap();
            upload1.set_states(upload1_changes);
        }
    }

    #[test]
    fn test_retranmission() {
        let (mut upload1, mut _download1) = YatcpBuilder {
            max_local_receiving_queue_len: 2,
            re_tx_timeout: time::Duration::from_secs(0),
        }
        .build();
        let (mut upload2, mut download2) = YatcpBuilder {
            max_local_receiving_queue_len: 2,
            re_tx_timeout: time::Duration::from_secs(0),
        }
        .build();

        // push: 1 -> 2
        {
            let buf = vec![0, 1, 2];
            let rdr = BufRdr::from_bytes(buf);
            upload1.to_send(rdr);

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
            let upload2_changes = download2.input(inflight).unwrap();
            upload2.set_states(upload2_changes);

            let recv2 = download2.recv().unwrap();
            assert_eq!(recv2.data(), vec![0, 1, 2]);
        }
        // ack: 1 <- 2
        {
            let mut inflight = OwnedBufWtr::new(1024, 0);
            let is_written = upload2.append_packet_to_and_if_written(&mut inflight);
            assert!(is_written);

            //                               rwnd] [     nack] [      seq] [cmd
            assert_eq!(inflight.data(), vec![0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

            // dropped
        }
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
