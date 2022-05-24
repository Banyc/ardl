use std::{
    net::{SocketAddr, UdpSocket},
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use crate_yatcp::{
    protocols::yatcp::{frag_hdr::PUSH_HDR_LEN, packet_hdr::PACKET_HDR_LEN},
    utils::{BufRdr, BufWtr, OwnedBufWtr},
    yatcp::{
        yatcp_download::YatcpDownload, yatcp_upload::YatcpUpload, SetUploadStates, YatcpBuilder,
    },
};

// const MTU: usize = 512;
const MTU: usize = PACKET_HDR_LEN + PUSH_HDR_LEN + 1;
const FLUSH_INTERVAL_MS: u64 = 10;
const LISTEN_ADDR: &str = "0.0.0.0:19479";
const MAX_LOCAL_RWND_LEN: usize = 2;

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
    }
    .build();

    // spawn threads
    let mut threads = Vec::new();
    let uploading_messaging_tx1 = Arc::clone(&uploading_messaging_tx);
    let thread = thread::spawn(move || {
        yatcp_downloading(
            yatcp_download,
            downloading_messaging_rx,
            uploading_messaging_tx1,
        )
    });
    threads.push(thread);
    let connection1 = Arc::clone(&listener);
    let thread = thread::spawn(move || {
        yatcp_uploading(connection1, yatcp_upload, uploading_messaging_rx);
    });
    threads.push(thread);
    let uploading_messaging_tx1 = Arc::clone(&uploading_messaging_tx);
    let thread = thread::spawn(move || timer(uploading_messaging_tx1));
    threads.push(thread);
    let connection1 = Arc::clone(&listener);
    let downloading_messaging_tx1 = Arc::clone(&downloading_messaging_tx);
    let thread = thread::spawn(move || socket_recving(connection1, downloading_messaging_tx1));
    threads.push(thread);

    // thread sleeps forever
    for thread in threads {
        thread.join().unwrap();
    }
}

fn timer(uploading_messaging_tx: Arc<mpsc::SyncSender<UploadingMessaging>>) {
    loop {
        thread::sleep(Duration::from_millis(FLUSH_INTERVAL_MS));
        uploading_messaging_tx
            .send(UploadingMessaging::Flush)
            .unwrap();
    }
}

fn yatcp_uploading(
    listener: Arc<UdpSocket>,
    mut upload: YatcpUpload,
    messaging: mpsc::Receiver<UploadingMessaging>,
) {
    let mut remote_addr_ = None;
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            UploadingMessaging::SetUploadStates(x) => {
                upload.set_states(x).unwrap();
            }
            UploadingMessaging::Flush => {
                let mut wtr = OwnedBufWtr::new(MTU, 0);
                let is_written = upload.append_packet_to_and_if_written(&mut wtr);
                if !is_written {
                    continue;
                }

                listener.send_to(wtr.data(), remote_addr_.unwrap()).unwrap();
            }
            UploadingMessaging::ToSend(rdr, remote_addr) => {
                upload.to_send(rdr);
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
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            DownloadingMessaging::ConnRecv(wtr, remote_addr) => {
                let rdr = BufRdr::from_wtr(wtr);
                let set_upload_states = match download.input(rdr) {
                    Ok(x) => x,
                    Err(_) => todo!(),
                };
                uploading_messaging_tx
                    .send(UploadingMessaging::SetUploadStates(set_upload_states))
                    .unwrap();

                let mut buf = Vec::new();
                while let Some(frag) = download.recv() {
                    buf.extend_from_slice(frag.data());
                }

                if !buf.is_empty() {
                    println!("{}, {:X?}", String::from_utf8_lossy(&buf), buf);

                    uploading_messaging_tx
                        .send(UploadingMessaging::ToSend(
                            BufRdr::from_bytes(buf),
                            remote_addr,
                        ))
                        .unwrap();
                }
            }
        }
    }
}

fn socket_recving(
    listener: Arc<UdpSocket>,
    downloading_messaging: Arc<mpsc::SyncSender<DownloadingMessaging>>,
) {
    loop {
        let mut buf = vec![0; MTU];
        let (len, remote_addr) = listener.recv_from(&mut buf).unwrap();

        let wtr = OwnedBufWtr::from_bytes(buf, 0, len);

        downloading_messaging
            .send(DownloadingMessaging::ConnRecv(wtr, remote_addr))
            .unwrap();
    }
}

enum UploadingMessaging {
    SetUploadStates(SetUploadStates),
    Flush,
    ToSend(BufRdr, SocketAddr),
}

enum DownloadingMessaging {
    ConnRecv(OwnedBufWtr, SocketAddr),
}
