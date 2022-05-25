use std::{
    io,
    net::UdpSocket,
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
const STAT_INTERVAL_S: u64 = 10;
const LISTEN_ADDR: &str = "0.0.0.0:19479";
const MAX_LOCAL_RWND_LEN: usize = 2;

fn main() {
    // socket
    let connection = UdpSocket::bind("0.0.0.0:0").unwrap();
    connection.connect(LISTEN_ADDR).unwrap();
    println!("Binding to {}", connection.local_addr().unwrap());
    let connection = Arc::new(connection);

    // channels
    let (uploading_messaging_tx, uploading_messaging_rx) = mpsc::sync_channel(0);
    let uploading_messaging_tx = Arc::new(uploading_messaging_tx);
    let (downloading_messaging_tx, downloading_messaging_rx) = mpsc::sync_channel(0);
    let downloading_messaging_tx = Arc::new(downloading_messaging_tx);

    // yatcp
    let (yatcp_upload, yatcp_download) = YatcpBuilder {
        max_local_receiving_queue_len: MAX_LOCAL_RWND_LEN,
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
        let connection1 = Arc::clone(&connection);
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
        let connection1 = Arc::clone(&connection);
        let downloading_messaging_tx1 = Arc::clone(&downloading_messaging_tx);
        let thread = thread::spawn(move || socket_recving(connection1, downloading_messaging_tx1));
        threads.push(thread);
    }

    // stdin
    loop {
        let mut text = String::new();
        io::stdin().read_line(&mut text).unwrap();
        if text.ends_with("\n") {
            text.truncate(text.len() - 1);
        }
        let bytes = text.into_bytes();
        let byte_len = bytes.len();
        let wtr = OwnedBufWtr::from_bytes(bytes, 0, byte_len);
        uploading_messaging_tx
            .send(UploadingMessaging::ToSend(BufRdr::from_wtr(wtr)))
            .unwrap();
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
    connection: Arc<UdpSocket>,
    mut upload: YatcpUpload,
    messaging: mpsc::Receiver<UploadingMessaging>,
) {
    let mut old_stat = None;
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            UploadingMessaging::SetUploadStates(x) => {
                upload.set_state(x).unwrap();
            }
            UploadingMessaging::Flush => {
                let mut wtr = OwnedBufWtr::new(MTU, 0);
                let is_written = upload.append_packet_to_and_if_written(&mut wtr);
                if !is_written {
                    continue;
                }

                connection.send(wtr.data()).unwrap();
            }
            UploadingMessaging::ToSend(rdr) => {
                upload.to_send(rdr);
            }
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
                    Err(e) => {
                        println!("err: download.input ({:?})", e);
                        continue;
                        // todo!()
                    }
                };
                uploading_messaging_tx
                    .send(UploadingMessaging::SetUploadStates(set_upload_states))
                    .unwrap();

                let mut buf = Vec::new();
                while let Some(frag) = download.recv() {
                    assert!(frag.data().len() > 0);

                    buf.extend_from_slice(frag.data());
                }

                if !buf.is_empty() {
                    println!("{}, {:X?}", String::from_utf8_lossy(&buf), buf);
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
                        println!("{}. Upload: {:?}", time, stat);
                    }
                }
                old_stat = Some(stat);
            }
        }
    }
}

fn socket_recving(
    connection: Arc<UdpSocket>,
    downloading_messaging: Arc<mpsc::SyncSender<DownloadingMessaging>>,
) {
    loop {
        let mut buf = vec![0; MTU];
        let len = connection.recv(&mut buf).unwrap();

        let wtr = OwnedBufWtr::from_bytes(buf, 0, len);

        downloading_messaging
            .send(DownloadingMessaging::ConnRecv(wtr))
            .unwrap();
    }
}

enum UploadingMessaging {
    SetUploadStates(SetUploadState),
    Flush,
    ToSend(BufRdr),
    PrintStat,
}

enum DownloadingMessaging {
    ConnRecv(OwnedBufWtr),
    PrintStat,
}
