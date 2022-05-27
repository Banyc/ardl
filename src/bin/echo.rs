use std::{
    net::{SocketAddr, UdpSocket},
    sync::{mpsc, Arc},
    thread,
    time::{self, Duration, SystemTime},
};

use crate_yatcp::{
    protocols::yatcp::{frag_hdr::PUSH_HDR_LEN, packet_hdr::PACKET_HDR_LEN},
    utils::{BufRdr, BufWtr, OwnedBufWtr},
    yatcp::{
        yatcp_download::YatcpDownload, yatcp_upload::YatcpUpload, SetUploadState, YatcpBuilder,
    },
};

// const MTU: usize = 512;
const MTU: usize = PACKET_HDR_LEN + PUSH_HDR_LEN + 1;
const FLUSH_INTERVAL_MS: u64 = 10;
const STAT_INTERVAL_S: u64 = 1;
const LISTEN_ADDR: &str = "0.0.0.0:19479";
const MAX_LOCAL_RWND_LEN: usize = 2;
const NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT: usize = 0;
const RATIO_RTO_TO_ONE_RTT: f64 = 1.5;
const TO_SEND_BYTE_CAPACITY: usize = 1024 * 64;

fn main() {
    // socket
    let listener = UdpSocket::bind(LISTEN_ADDR).unwrap();
    println!("listening on {}", listener.local_addr().unwrap());
    let listener = Arc::new(listener);

    // channels
    let (uploading_messaging_tx, uploading_messaging_rx) = mpsc::sync_channel(0);
    let uploading_messaging_tx = Arc::new(uploading_messaging_tx);
    let (downloading_messaging_tx, downloading_messaging_rx) = mpsc::sync_channel(0);
    let downloading_messaging_tx = Arc::new(downloading_messaging_tx);

    // yatcp
    let (yatcp_upload, yatcp_download) = YatcpBuilder {
        max_local_receiving_queue_len: MAX_LOCAL_RWND_LEN,
        nack_duplicate_threshold_to_activate_fast_retransmit:
            NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT,
        ratio_rto_to_one_rtt: RATIO_RTO_TO_ONE_RTT,
        to_send_byte_capacity: TO_SEND_BYTE_CAPACITY,
    }
    .build();

    // spawn threads
    let mut threads = Vec::new();
    {
        let uploading_messaging_tx1 = Arc::clone(&uploading_messaging_tx);
        let thread = thread::spawn(move || {
            yatcp_downloading(
                yatcp_download,
                downloading_messaging_rx,
                uploading_messaging_tx1,
            )
        });
        threads.push(thread);
    }
    {
        let connection1 = Arc::clone(&listener);
        let thread = thread::spawn(move || {
            yatcp_uploading(connection1, yatcp_upload, uploading_messaging_rx);
        });
        threads.push(thread);
    }
    {
        let uploading_messaging_tx1 = Arc::clone(&uploading_messaging_tx);
        let thread = thread::spawn(move || flush_timer(uploading_messaging_tx1));
        threads.push(thread);
    }
    {
        let uploading_messaging_tx1 = Arc::clone(&uploading_messaging_tx);
        let downloading_messaging_tx1 = Arc::clone(&downloading_messaging_tx);
        let thread =
            thread::spawn(move || stat_timer(uploading_messaging_tx1, downloading_messaging_tx1));
        threads.push(thread);
    }
    {
        let connection1 = Arc::clone(&listener);
        let uploading_messaging_tx1 = Arc::clone(&uploading_messaging_tx);
        let downloading_messaging_tx1 = Arc::clone(&downloading_messaging_tx);
        let thread = thread::spawn(move || {
            socket_recving(
                connection1,
                uploading_messaging_tx1,
                downloading_messaging_tx1,
            )
        });
        threads.push(thread);
    }

    // thread sleeps forever
    for thread in threads {
        thread.join().unwrap();
    }
}

fn flush_timer(uploading_messaging_tx: Arc<mpsc::SyncSender<UploadingMessaging>>) {
    loop {
        thread::sleep(Duration::from_millis(FLUSH_INTERVAL_MS));
        uploading_messaging_tx
            .send(UploadingMessaging::Flush)
            .unwrap();
    }
}

fn stat_timer(
    uploading_messaging_tx: Arc<mpsc::SyncSender<UploadingMessaging>>,
    downloading_messaging: Arc<mpsc::SyncSender<DownloadingMessaging>>,
) {
    loop {
        thread::sleep(Duration::from_secs(STAT_INTERVAL_S));
        uploading_messaging_tx
            .send(UploadingMessaging::PrintStat)
            .unwrap();
        thread::sleep(Duration::from_secs(STAT_INTERVAL_S));
        downloading_messaging
            .send(DownloadingMessaging::PrintStat)
            .unwrap();
    }
}

fn yatcp_uploading(
    listener: Arc<UdpSocket>,
    mut upload: YatcpUpload,
    messaging: mpsc::Receiver<UploadingMessaging>,
) {
    let mut old_stat = None;
    let mut remote_addr_ = None;
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            UploadingMessaging::SetUploadState(state) => {
                upload.set_state(state).unwrap();
            }
            UploadingMessaging::Flush => {
                if let None = remote_addr_ {
                    continue;
                }

                let mut wtr = OwnedBufWtr::new(MTU, 0);
                let is_written = upload.append_packet_to_and_if_written(&mut wtr);
                if !is_written {
                    continue;
                }

                listener.send_to(wtr.data(), remote_addr_.unwrap()).unwrap();
            }
            UploadingMessaging::ToSend(rdr, responser) => match upload.to_send(rdr) {
                Ok(()) => responser.send(UploadingToSendResponse::Ok).unwrap(),
                Err(e) => responser.send(UploadingToSendResponse::Err(e.0)).unwrap(),
            },
            UploadingMessaging::PrintStat => {
                let stat = upload.stat();
                if let Some(old_stat) = old_stat {
                    if old_stat != stat {
                        let time = SystemTime::now()
                            .duration_since(time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis();
                        println!("{}. Upload: {:?}", time, stat);
                    }
                }
                old_stat = Some(stat);
            }
            UploadingMessaging::SetRemoteAddr(remote_addr) => {
                remote_addr_ = Some(remote_addr);
            }
        }
    }
}

fn yatcp_downloading(
    mut download: YatcpDownload,
    messaging: mpsc::Receiver<DownloadingMessaging>,
    uploading_messaging_tx: Arc<mpsc::SyncSender<UploadingMessaging>>,
) {
    let mut old_stat = None;
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            DownloadingMessaging::ConnRecv(wtr) => {
                let rdr = BufRdr::from_wtr(wtr);
                let set_upload_states = match download.input(rdr) {
                    Ok(x) => x,
                    Err(_) => todo!(),
                };
                uploading_messaging_tx
                    .send(UploadingMessaging::SetUploadState(set_upload_states))
                    .unwrap();

                let mut buf = Vec::new();
                while let Some(frag) = download.recv() {
                    buf.extend_from_slice(frag.data());
                }

                if !buf.is_empty() {
                    println!("{}, {:X?}", String::from_utf8_lossy(&buf), buf);

                    let rdr = BufRdr::from_bytes(buf);
                    block_sending(rdr, &uploading_messaging_tx);
                }
            }
            DownloadingMessaging::PrintStat => {
                let stat = download.stat();
                if let Some(old_stat) = old_stat {
                    if old_stat != stat {
                        let time = SystemTime::now()
                            .duration_since(time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis();
                        println!("{}. Download: {:?}", time, stat);
                    }
                }
                old_stat = Some(stat);
            }
        }
    }
}

fn socket_recving(
    listener: Arc<UdpSocket>,
    uploading_messaging: Arc<mpsc::SyncSender<UploadingMessaging>>,
    downloading_messaging: Arc<mpsc::SyncSender<DownloadingMessaging>>,
) {
    loop {
        let mut buf = vec![0; MTU];
        let (len, remote_addr) = listener.recv_from(&mut buf).unwrap();

        let wtr = OwnedBufWtr::from_bytes(buf, 0, len);

        uploading_messaging
            .send(UploadingMessaging::SetRemoteAddr(remote_addr))
            .unwrap();
        downloading_messaging
            .send(DownloadingMessaging::ConnRecv(wtr))
            .unwrap();
    }
}

enum UploadingMessaging {
    SetUploadState(SetUploadState),
    Flush,
    ToSend(BufRdr, mpsc::SyncSender<UploadingToSendResponse>),
    PrintStat,
    SetRemoteAddr(SocketAddr),
}

enum DownloadingMessaging {
    ConnRecv(OwnedBufWtr),
    PrintStat,
}

enum UploadingToSendResponse {
    Ok,
    Err(BufRdr),
}

fn block_sending(rdr: BufRdr, uploading_messaging_tx: &mpsc::SyncSender<UploadingMessaging>) {
    let mut some_rdr = Some(rdr);
    loop {
        let rdr = some_rdr.take().unwrap();
        let (responser, receiver) = mpsc::sync_channel(1);
        uploading_messaging_tx
            .send(UploadingMessaging::ToSend(rdr, responser))
            .unwrap();
        let res = receiver.recv().unwrap();
        match res {
            UploadingToSendResponse::Ok => break,
            UploadingToSendResponse::Err(rdr) => {
                some_rdr = Some(rdr);
            }
        }
        thread::sleep(Duration::from_millis(FLUSH_INTERVAL_MS * 3));
    }
}
