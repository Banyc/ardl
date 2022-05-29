use std::{
    io,
    net::UdpSocket,
    sync::{mpsc, Arc},
    thread,
    time::{self, Duration, SystemTime},
};

use ardl::{
    layer::{Builder, Downloader, IObserver, SetUploadState, Uploader},
    protocol::{frag_hdr::PUSH_HDR_LEN, packet_hdr::PACKET_HDR_LEN},
    utils::buf::{BufSlice, BufWtr, OwnedBufWtr},
};

// const MTU: usize = 512;
const MTU: usize = PACKET_HDR_LEN + PUSH_HDR_LEN + 1;
const FLUSH_INTERVAL_MS: u64 = 10;
const STAT_INTERVAL_S: u64 = 1;
const LISTEN_ADDR: &str = "0.0.0.0:19479";
const LOCAL_RECV_BUF_LEN: usize = 2;
const NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT: usize = 0;
const RATIO_RTO_TO_ONE_RTT: f64 = 1.5;
// const TO_SEND_QUEUE_LEN_CAP: usize = 1024 * 64;
const TO_SEND_QUEUE_LEN_CAP: usize = 1;
const MAX_SWND_SIZE: usize = usize::MAX;

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
    let (on_send_available_tx, on_send_available_rx) = mpsc::sync_channel(1);

    // layer
    let (mut uploader, downloader) = Builder {
        local_recv_buf_len: LOCAL_RECV_BUF_LEN,
        nack_duplicate_threshold_to_activate_fast_retransmit:
            NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT,
        ratio_rto_to_one_rtt: RATIO_RTO_TO_ONE_RTT,
        to_send_queue_len_cap: TO_SEND_QUEUE_LEN_CAP,
        swnd_size_cap: MAX_SWND_SIZE,
    }
    .build();

    // on send available
    let observer = OnSendAvailable {
        tx: on_send_available_tx,
    };
    let observer = Arc::new(observer);
    let weak_observer = Arc::downgrade(&observer);
    uploader.set_on_send_available(Some(weak_observer));

    // spawn threads
    let mut threads = Vec::new();
    {
        let uploading_messaging_tx1 = Arc::clone(&uploading_messaging_tx);
        let thread = thread::spawn(move || {
            downloading(
                downloader,
                downloading_messaging_rx,
                uploading_messaging_tx1,
            )
        });
        threads.push(thread);
    }
    {
        let connection1 = Arc::clone(&connection);
        let thread = thread::spawn(move || {
            uploading(connection1, uploader, uploading_messaging_rx);
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
        let slice = BufSlice::from_wtr(wtr);

        block_sending(slice, &uploading_messaging_tx, &on_send_available_rx);
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

fn uploading(
    connection: Arc<UdpSocket>,
    mut uploader: Uploader,
    messaging: mpsc::Receiver<UploadingMessaging>,
) {
    let mut old_stat = None;
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            UploadingMessaging::SetUploadStates(x) => {
                uploader.set_state(x).unwrap();
            }
            UploadingMessaging::Flush => {
                let mut wtr = OwnedBufWtr::new(MTU, 0);
                if let Ok(()) = uploader.output_packet(&mut wtr) {
                    connection.send(wtr.data()).unwrap();
                }
            }
            UploadingMessaging::ToSend(slice, responser) => match uploader.to_send(slice) {
                Ok(()) => responser.send(UploadingToSendResponse::Ok).unwrap(),
                Err(e) => responser.send(UploadingToSendResponse::Err(e.0)).unwrap(),
            },
            UploadingMessaging::PrintStat => {
                let stat = uploader.stat();
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

fn downloading(
    mut downloader: Downloader,
    messaging: mpsc::Receiver<DownloadingMessaging>,
    uploading_messaging_tx: Arc<mpsc::SyncSender<UploadingMessaging>>,
) {
    let mut old_stat = None;
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            DownloadingMessaging::ConnRecv(wtr) => {
                let rdr = BufSlice::from_wtr(wtr);
                let set_upload_states = match downloader.input_packet(rdr) {
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
                while let Some(slice) = downloader.recv() {
                    assert!(slice.data().len() > 0);

                    buf.extend_from_slice(slice.data());
                }

                if !buf.is_empty() {
                    println!("{}, {:X?}", String::from_utf8_lossy(&buf), buf);
                }
            }
            DownloadingMessaging::PrintStat => {
                let stat = downloader.stat();
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
    ToSend(BufSlice, mpsc::SyncSender<UploadingToSendResponse>),
    PrintStat,
}

enum DownloadingMessaging {
    ConnRecv(OwnedBufWtr),
    PrintStat,
}

enum UploadingToSendResponse {
    Ok,
    Err(BufSlice),
}

fn block_sending(
    slice: BufSlice,
    uploading_messaging_tx: &mpsc::SyncSender<UploadingMessaging>,
    on_send_available_rx: &mpsc::Receiver<()>,
) {
    let mut some_slice = Some(slice);
    loop {
        let slice = some_slice.take().unwrap();
        let (responser, receiver) = mpsc::sync_channel(1);
        uploading_messaging_tx
            .send(UploadingMessaging::ToSend(slice, responser))
            .unwrap();
        let res = receiver.recv().unwrap();
        match res {
            UploadingToSendResponse::Ok => break,
            UploadingToSendResponse::Err(slice) => {
                some_slice = Some(slice);
            }
        }
        // println!("got blocked");
        on_send_available_rx.recv().unwrap();
        // println!("got unblocked");
    }
}

struct OnSendAvailable {
    tx: mpsc::SyncSender<()>,
}

impl IObserver for OnSendAvailable {
    fn notify(&self) {
        let _ = self.tx.try_send(());
    }
}
