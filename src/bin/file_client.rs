use ardl::{
    layer::{Builder, Downloader, IObserver, SetUploadState, Uploader},
    utils::buf::{BufSlice, BufWtr, OwnedBufWtr},
};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    net::UdpSocket,
    path::PathBuf,
    str::FromStr,
    sync::{mpsc, Arc},
    thread,
    time::{self, Duration, Instant, SystemTime},
};

const MTU: usize = 1300;
const FLUSH_INTERVAL_MS: u64 = 1;
const STAT_INTERVAL_S: u64 = 1;
const LISTEN_ADDR: &str = "0.0.0.0:19479";
const LOCAL_RECV_BUF_LEN: usize = 1024;
const RATIO_RTO_TO_ONE_RTT: f64 = 1.5;
// const TO_SEND_QUEUE_LEN_CAP: usize = 1024 * 64;
const TO_SEND_QUEUE_LEN_CAP: usize = 1024;
const SWND_SIZE_CAP: usize = 1024;
const SOURCE_FILE_NAME: &str = "Free_Test_Data_10MB_MP4.upload.mp4";
const DESTINATION_FILE_NAME: &str = "Free_Test_Data_10MB_MP4.download.mp4";
static NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT: usize = SWND_SIZE_CAP * 1 / 2;

fn main() {
    // file
    let source = PathBuf::from_str(SOURCE_FILE_NAME).unwrap();
    let destination = PathBuf::from_str(DESTINATION_FILE_NAME).unwrap();
    let mut source = File::open(source).unwrap();
    let source_size = source.metadata().unwrap().len() as usize;
    match fs::remove_file(&destination) {
        Ok(_) => (),
        Err(e) => match e.kind() {
            io::ErrorKind::NotFound => (),
            _ => panic!(),
        },
    }
    let destination = File::create(destination).unwrap();

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
    let (processing_messaging_tx, processing_messaging_rx) = mpsc::sync_channel(0);
    let processing_messaging_tx = Arc::new(processing_messaging_tx);
    let (on_send_available_tx, on_send_available_rx) = mpsc::sync_channel(1);
    let (on_destination_available_tx, on_destination_available_rx) = mpsc::sync_channel(1);

    // layer
    let (mut uploader, downloader) = Builder {
        local_recv_buf_len: LOCAL_RECV_BUF_LEN,
        nack_duplicate_threshold_to_activate_fast_retransmit:
            NACK_DUPLICATE_THRESHOLD_TO_ACTIVATE_FAST_RETRANSMIT,
        ratio_rto_to_one_rtt: RATIO_RTO_TO_ONE_RTT,
        to_send_queue_len_cap: TO_SEND_QUEUE_LEN_CAP,
        swnd_size_cap: SWND_SIZE_CAP,
        mtu: MTU,
    }
    .build()
    .unwrap();

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
        let processing_messaging_tx1 = Arc::clone(&processing_messaging_tx);
        let thread = thread::spawn(move || {
            downloading(
                downloader,
                downloading_messaging_rx,
                uploading_messaging_tx1,
                processing_messaging_tx1,
            )
        });
        threads.push(thread);
    }
    {
        let downloading_messaging_tx1 = Arc::clone(&downloading_messaging_tx);
        let thread = thread::spawn(move || {
            processing(
                processing_messaging_rx,
                downloading_messaging_tx1,
                destination,
                source_size,
                on_destination_available_tx,
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
        let thread =
            thread::spawn(move || socket_receiving(connection1, downloading_messaging_tx1));
        threads.push(thread);
    }

    // send source
    let before_send = Instant::now();
    loop {
        let mut wtr = OwnedBufWtr::new(1024 * 64, 0);
        let len = source.read(wtr.back_free_space()).unwrap();
        if len == 0 {
            break;
        }
        wtr.grow_back(len).unwrap();

        let slice = BufSlice::from_wtr(wtr);

        block_sending(slice, &uploading_messaging_tx, &on_send_available_rx);
    }
    let send_duration = Instant::now().duration_since(before_send);
    println!(
        "main: done reading file. Speed: {:.2} mB/s",
        source_size as f64 / send_duration.as_secs_f64() / 1000.0 / 1000.0
    );
    on_destination_available_rx.recv().unwrap();
    println!("main: done receiving file");

    // flush all acks
    thread::sleep(Duration::from_secs(1));

    // verify integrity
    // TODO
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
            UploadingMessaging::SetUploadState(x) => {
                uploader.set_state(x, &Instant::now()).unwrap();
                output(&mut uploader, &connection);
            }
            UploadingMessaging::Flush => {
                output(&mut uploader, &connection);
            }
            UploadingMessaging::ToSend(slice, responser) => match uploader.write(slice) {
                Ok(()) => {
                    responser.send(UploadingToSendResponse::Ok).unwrap();
                    output(&mut uploader, &connection);
                }
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
    processing_messaging_tx: Arc<mpsc::SyncSender<ProcessingMessaging>>,
) {
    let mut old_stat = None;
    let mut is_processing_free = false;
    loop {
        let msg = messaging.recv().unwrap();
        match msg {
            DownloadingMessaging::ConnRecv(wtr) => {
                let rdr = BufSlice::from_wtr(wtr);
                let set_upload_state = match downloader.write(rdr) {
                    Ok(x) => x,
                    Err(e) => {
                        println!("err: download.input ({:?})", e);
                        continue;
                        // todo!()
                    }
                };
                uploading_messaging_tx
                    .send(UploadingMessaging::SetUploadState(set_upload_state))
                    .unwrap();
                if is_processing_free {
                    if let Some(slice) = downloader.emit() {
                        processing_messaging_tx
                            .send(ProcessingMessaging::Recv(slice))
                            .unwrap();
                        is_processing_free = false;
                    }
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
            DownloadingMessaging::ProcessingIsFree => {
                is_processing_free = true;
                if let Some(slice) = downloader.emit() {
                    processing_messaging_tx
                        .send(ProcessingMessaging::Recv(slice))
                        .unwrap();
                    is_processing_free = false;
                }
            }
        }
    }
}

// basically a channel of size one
fn processing(
    messaging: mpsc::Receiver<ProcessingMessaging>,
    downloading_messaging_tx: Arc<mpsc::SyncSender<DownloadingMessaging>>,
    destination: fs::File,
    source_size: usize,
    on_destination_available_tx: mpsc::SyncSender<()>,
) {
    let mut destination = Some(destination);
    let mut bytes_written_so_far = 0;
    let mut last_print = Instant::now();
    let print_duration = Duration::from_secs(1);
    let mut speed = 0.0;
    let mut last_recv = Instant::now();
    let mut last_bytes_written_so_far = 0;
    // let alpha = 1.0 / 8.0;
    let alpha = 1.0 / 1.0;
    loop {
        downloading_messaging_tx
            .send(DownloadingMessaging::ProcessingIsFree)
            .unwrap();
        let msg = messaging.recv().unwrap();
        match msg {
            ProcessingMessaging::Recv(slice) => {
                let mut file = destination.take().unwrap();
                file.write(slice.data()).unwrap();
                bytes_written_so_far += slice.data().len();

                if Instant::now().duration_since(last_print) > print_duration {
                    let this_speed = (bytes_written_so_far - last_bytes_written_so_far) as f64
                        / Instant::now().duration_since(last_recv).as_secs_f64();
                    speed = (1.0 - alpha) * speed + alpha * this_speed;
                    last_recv = Instant::now();
                    last_bytes_written_so_far = bytes_written_so_far;

                    println!(
                        "Progress: {:.2}%. Speed: {:.2} kB/s",
                        bytes_written_so_far as f64 / source_size as f64 * 100.0,
                        speed / 1000.0,
                    );
                    last_print = Instant::now();
                }

                if bytes_written_so_far == source_size {
                    drop(file);
                    on_destination_available_tx.send(()).unwrap();
                    continue;
                }
                destination = Some(file);
                assert!(bytes_written_so_far < source_size);
            }
        }
    }
}

fn socket_receiving(
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
    SetUploadState(SetUploadState),
    Flush,
    ToSend(BufSlice, mpsc::SyncSender<UploadingToSendResponse>),
    PrintStat,
}

enum DownloadingMessaging {
    ConnRecv(OwnedBufWtr),
    PrintStat,
    ProcessingIsFree,
}

enum ProcessingMessaging {
    Recv(BufSlice),
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

fn output(uploader: &mut Uploader, connection: &Arc<UdpSocket>) {
    let mut wtr = OwnedBufWtr::new(MTU, 0);
    let wtr_data_len = wtr.data_len();
    let packets = uploader.emit(&Instant::now());
    for packet in packets {
        packet.append_to(&mut wtr).unwrap();
        match connection.send(wtr.data()) {
            Ok(_) => (),
            Err(e) => {
                println!("uploading: {}", e);
                break;
            }
        }
        wtr.shrink_back(wtr.data_len() - wtr_data_len).unwrap();
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
