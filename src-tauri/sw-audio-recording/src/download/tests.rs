use super::transfer::{
    download_file, lock_part_file, part_path_for, part_path_private_to_this_process, DownloadSpec,
};
use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn unique_test_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "sw-audio-recording-test-{tag}-{}-{}",
        std::process::id(),
        n
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), b"x").unwrap();
}

fn full_content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

async fn read_range_start(stream: &mut TcpStream) -> Option<u64> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))?;
    let value = line.split_once(':')?.1.trim();
    let spec = value.strip_prefix("bytes=")?;
    spec.trim_end_matches('-').parse::<u64>().ok()
}

async fn write_response(
    stream: &mut TcpStream,
    status_line: &str,
    declared_len: usize,
    body: &[u8],
) {
    let head =
        format!("{status_line}\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.shutdown().await;
}

#[test]
fn an_existing_fp16_install_is_still_reported_ready_after_the_int8_switch() {
    let dir = unique_test_dir("fp16-ready");
    touch(&dir, "encoder-model.fp16.onnx");
    touch(&dir, "decoder_joint-model.fp16.onnx");
    touch(&dir, "vocab.txt");
    touch(&dir, "config.json");
    touch(&dir, "silero_vad.onnx");

    assert!(models_ready(&dir));
    assert_eq!(installed_precision(&dir), Some(ModelPrecision::Fp16));
}

#[test]
fn a_fresh_int8_only_install_is_reported_ready() {
    let dir = unique_test_dir("int8-ready");
    touch(&dir, "encoder-model.int8.onnx");
    touch(&dir, "decoder_joint-model.int8.onnx");
    touch(&dir, "vocab.txt");
    touch(&dir, "config.json");
    touch(&dir, "silero_vad.onnx");

    assert!(models_ready(&dir));
    assert_eq!(installed_precision(&dir), Some(ModelPrecision::Int8));
}

#[test]
fn an_install_missing_the_vad_model_is_not_reported_ready() {
    let dir = unique_test_dir("missing-vad");
    touch(&dir, "encoder-model.fp16.onnx");
    touch(&dir, "decoder_joint-model.fp16.onnx");
    touch(&dir, "vocab.txt");
    touch(&dir, "config.json");

    assert!(!models_ready(&dir));
}

#[test]
fn an_install_with_neither_variant_complete_is_not_reported_ready() {
    let dir = unique_test_dir("neither-complete");
    touch(&dir, "encoder-model.fp16.onnx");
    touch(&dir, "vocab.txt");
    touch(&dir, "config.json");
    touch(&dir, "silero_vad.onnx");

    assert!(!models_ready(&dir));
}

#[test]
fn a_partial_download_marker_is_not_mistaken_for_a_complete_file() {
    let dir = unique_test_dir("partial-not-complete");
    touch(&dir, ".encoder-model.int8.onnx.part");
    touch(&dir, "decoder_joint-model.int8.onnx");
    touch(&dir, "vocab.txt");
    touch(&dir, "config.json");
    touch(&dir, "silero_vad.onnx");

    assert!(!models_ready(&dir));
    assert!(!model_exists(&dir));
}

#[test]
fn models_ready_verified_rejects_files_with_wrong_content() {
    let dir = unique_test_dir("verified-wrong-hash");
    touch(&dir, "encoder-model.fp16.onnx");
    touch(&dir, "decoder_joint-model.fp16.onnx");
    touch(&dir, "vocab.txt");
    touch(&dir, "config.json");
    touch(&dir, "silero_vad.onnx");

    assert!(!models_ready_verified(&dir));
}

#[test]
fn pinned_revision_differs_between_fp16_and_int8() {
    assert_ne!(
        pinned_revision(ModelPrecision::Fp16),
        pinned_revision(ModelPrecision::Int8)
    );
}

#[test]
fn pinned_revision_is_stable_across_repeated_calls() {
    assert_eq!(
        pinned_revision(ModelPrecision::Fp16),
        pinned_revision(ModelPrecision::Fp16)
    );
    assert_eq!(
        pinned_revision(ModelPrecision::Int8),
        pinned_revision(ModelPrecision::Int8)
    );
}

#[test]
fn part_path_for_is_deterministic_and_hidden() {
    let dest = PathBuf::from("/some/dir/encoder-model.int8.onnx");
    let a = part_path_for(&dest);
    let b = part_path_for(&dest);
    assert_eq!(a, b);
    assert_eq!(
        a.file_name().unwrap().to_str().unwrap(),
        ".encoder-model.int8.onnx.part"
    );
}

#[tokio::test]
async fn an_interrupted_download_resumes_from_the_correct_byte_offset_via_range() {
    let content = full_content(200_000);
    let sha = sha256_hex(&content);
    let cutoff = content.len() * 2 / 3;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/encoder.bin");

    let content_for_server = content.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_range_start(&mut stream).await;
        write_response(
            &mut stream,
            "HTTP/1.1 200 OK",
            content_for_server.len(),
            &content_for_server[..cutoff],
        )
        .await;

        let (mut stream, _) = listener.accept().await.unwrap();
        let start = read_range_start(&mut stream)
            .await
            .expect("resumed request must send a Range header") as usize;
        assert!(start > 0 && start <= cutoff);
        write_response(
            &mut stream,
            "HTTP/1.1 206 Partial Content",
            content_for_server.len() - start,
            &content_for_server[start..],
        )
        .await;
    });

    let dir = unique_test_dir("resume-offset");
    let dest_path = dir.join("encoder.bin");
    let client = Client::new();

    let first = download_file(
        &client,
        DownloadSpec {
            url: &url,
            filename: "encoder.bin",
            expected_sha256: &sha,
        },
        &dest_path,
        0,
        1,
        &|_: DownloadProgress| {},
    )
    .await;
    assert!(first.is_err());

    let part = part_path_for(&dest_path);
    let resumed_offset = std::fs::metadata(&part).unwrap().len() as usize;
    assert!(
        resumed_offset > 0,
        "the interrupted transfer wrote no bytes"
    );
    assert!(resumed_offset <= cutoff);

    let progress_log: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let log_for_closure = progress_log.clone();
    let second = download_file(
        &client,
        DownloadSpec {
            url: &url,
            filename: "encoder.bin",
            expected_sha256: &sha,
        },
        &dest_path,
        0,
        1,
        &move |p: DownloadProgress| {
            log_for_closure.lock().unwrap().push(p.bytes_downloaded);
        },
    )
    .await;
    assert!(second.is_ok(), "{second:?}");
    assert!(!part.exists());
    assert_eq!(std::fs::read(&dest_path).unwrap(), content);

    {
        let log = progress_log.lock().unwrap();
        assert!(log.iter().all(|&b| b as usize >= resumed_offset));
        assert_eq!(*log.last().unwrap() as usize, content.len());
    }

    server.await.unwrap();
}

#[tokio::test]
async fn a_resumed_download_restarts_from_scratch_when_the_server_ignores_the_range_header() {
    let content = full_content(5_000);
    let sha = sha256_hex(&content);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/decoder.bin");

    let dir = unique_test_dir("ignore-range");
    let dest_path = dir.join("decoder.bin");
    let part = part_path_for(&dest_path);
    std::fs::write(&part, b"stale-partial-bytes-from-a-previous-attempt").unwrap();

    let content_for_server = content.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_range_start(&mut stream).await;
        write_response(
            &mut stream,
            "HTTP/1.1 200 OK",
            content_for_server.len(),
            &content_for_server,
        )
        .await;
    });

    let client = Client::new();
    let result = download_file(
        &client,
        DownloadSpec {
            url: &url,
            filename: "decoder.bin",
            expected_sha256: &sha,
        },
        &dest_path,
        0,
        1,
        &|_: DownloadProgress| {},
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
    assert!(!part.exists());
    assert_eq!(std::fs::read(&dest_path).unwrap(), content);

    server.await.unwrap();
}

#[tokio::test]
async fn a_corrupted_resume_is_deleted_and_not_treated_as_valid() {
    let content = full_content(10_000);
    let sha = sha256_hex(&content);
    let split = content.len() / 2;

    let dir = unique_test_dir("corrupt-resume");
    let dest_path = dir.join("model.bin");
    let part = part_path_for(&dest_path);
    std::fs::write(&part, &content[..split]).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/model.bin");

    let corrupted_tail: Vec<u8> = content[split..].iter().map(|b| b.wrapping_add(1)).collect();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let start = read_range_start(&mut stream).await.unwrap();
        assert_eq!(start as usize, split);
        write_response(
            &mut stream,
            "HTTP/1.1 206 Partial Content",
            corrupted_tail.len(),
            &corrupted_tail,
        )
        .await;
    });

    let client = Client::new();
    let result = download_file(
        &client,
        DownloadSpec {
            url: &url,
            filename: "model.bin",
            expected_sha256: &sha,
        },
        &dest_path,
        0,
        1,
        &|_: DownloadProgress| {},
    )
    .await;

    assert!(result.is_err());
    assert!(!part.exists());
    assert!(!dest_path.exists());

    server.await.unwrap();
}

#[tokio::test]
async fn a_complete_partial_file_rejected_with_416_is_discarded_and_downloaded_again() {
    let content = full_content(4_000);
    let sha = sha256_hex(&content);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/encoder.bin");

    let dir = unique_test_dir("range-not-satisfiable");
    let dest_path = dir.join("encoder.bin");
    let part = part_path_for(&dest_path);
    std::fs::write(&part, &content).unwrap();

    let content_for_server = content.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let start = read_range_start(&mut stream)
            .await
            .expect("the first attempt must ask to resume");
        assert_eq!(start as usize, content_for_server.len());
        write_response(&mut stream, "HTTP/1.1 416 Range Not Satisfiable", 0, b"").await;

        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(
            read_range_start(&mut stream).await,
            None,
            "the retry must ask for the whole file"
        );
        write_response(
            &mut stream,
            "HTTP/1.1 200 OK",
            content_for_server.len(),
            &content_for_server,
        )
        .await;
    });

    let client = Client::new();
    download_file(
        &client,
        DownloadSpec {
            url: &url,
            filename: "encoder.bin",
            expected_sha256: &sha,
        },
        &dest_path,
        0,
        1,
        &|_: DownloadProgress| {},
    )
    .await
    .expect("a 416 on a stale partial must not be fatal");

    assert_eq!(std::fs::read(&dest_path).unwrap(), content);
    assert!(!part.exists());

    server.await.unwrap();
}

#[test]
fn the_multilingual_pin_is_unchanged_by_the_english_model_being_added() {
    assert_eq!(
        pinned_revision(ModelPrecision::Fp16),
        pinned_revision_of(SpeechModel::ParakeetMultilingualFp16)
    );
    assert_eq!(
        pinned_revision(ModelPrecision::Int8),
        pinned_revision_of(SpeechModel::ParakeetMultilingualInt8)
    );
}

#[test]
fn the_english_model_pins_to_a_different_revision_than_the_multilingual_one() {
    assert_ne!(
        pinned_revision_of(SpeechModel::ParakeetEnglishInt8),
        pinned_revision_of(SpeechModel::ParakeetMultilingualInt8)
    );
}

#[test]
fn the_english_and_multilingual_models_never_share_a_directory() {
    assert_ne!(
        SpeechModel::ParakeetEnglishInt8.directory_name(),
        SpeechModel::ParakeetMultilingualInt8.directory_name(),
        "the two models use identical filenames, so only separate directories keep them apart"
    );
    assert_eq!(
        SpeechModel::ParakeetMultilingualFp16.directory_name(),
        SpeechModel::ParakeetMultilingualInt8.directory_name(),
        "both precisions of the multilingual model are interchangeable in one directory"
    );
}

#[test]
fn the_english_model_falls_back_to_our_own_mirror_but_the_multilingual_one_has_none() {
    let file = &SpeechModel::ParakeetEnglishInt8.files()[0];
    assert_eq!(
        mirror_url(SpeechModel::ParakeetEnglishInt8, file).as_deref(),
        Some("https://dl.stagewhisper.io/models/parakeet-tdt-ctc-110m-en/encoder-model.int8.onnx")
    );
    assert_eq!(
        mirror_url(
            SpeechModel::ParakeetMultilingualInt8,
            &SpeechModel::ParakeetMultilingualInt8.files()[0]
        ),
        None
    );
}

#[test]
fn every_mirrored_file_is_reachable_under_the_mirror_base() {
    for file in SpeechModel::ParakeetEnglishInt8.files() {
        let url = mirror_url(SpeechModel::ParakeetEnglishInt8, file)
            .expect("the english model is mirrored");
        assert!(url.starts_with("https://dl.stagewhisper.io/models/"));
        assert!(url.ends_with(file.filename));
    }
}

#[test]
fn an_english_install_is_only_reported_present_once_every_file_has_landed() {
    let dir = unique_test_dir("english-present");
    touch(&dir, "encoder-model.int8.onnx");
    touch(&dir, "decoder_joint-model.int8.onnx");
    touch(&dir, "vocab.txt");
    assert!(!model_files_present(&dir, SpeechModel::ParakeetEnglishInt8));

    touch(&dir, "config.json");
    touch(&dir, "silero_vad.onnx");
    assert!(model_files_present(&dir, SpeechModel::ParakeetEnglishInt8));
}

#[test]
fn a_second_downloader_of_the_same_file_never_writes_the_first_ones_partial_file() {
    let dir = unique_test_dir("concurrent-part");
    let dest = dir.join("encoder-model.int8.onnx");
    let shared = part_path_for(&dest);

    let first = lock_part_file(&shared).expect("the first downloader takes the shared part file");
    assert!(
        lock_part_file(&shared).is_none(),
        "a second downloader must not be handed the same partial file"
    );

    let private = part_path_private_to_this_process(&dest);
    assert_ne!(private, shared);
    assert!(private
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with(".encoder-model.int8.onnx.part."));

    drop(first);
    assert!(
        lock_part_file(&shared).is_some(),
        "the shared partial file is resumable again once the first downloader is done"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_multilingual_install_still_leaves_the_markers_an_older_desktop_build_reads() {
    assert_eq!(
        SpeechModel::ParakeetMultilingualFp16.variant_marker_older_builds_read(),
        Some("FP16")
    );
    assert_eq!(
        SpeechModel::ParakeetMultilingualInt8.variant_marker_older_builds_read(),
        Some("INT8 quantized")
    );
}

#[test]
fn the_english_model_writes_no_legacy_marker_because_no_older_build_reads_its_directory() {
    assert_eq!(
        SpeechModel::ParakeetEnglishInt8.variant_marker_older_builds_read(),
        None
    );
    assert_ne!(
        SpeechModel::ParakeetEnglishInt8.directory_name(),
        SpeechModel::ParakeetMultilingualFp16.directory_name()
    );
}

#[tokio::test]
async fn a_download_racing_another_process_leaves_that_partial_file_untouched_and_still_succeeds() {
    let content = full_content(40_000);
    let sha = sha256_hex(&content);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/encoder.bin");

    let content_for_server = content.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_range_start(&mut stream).await;
        write_response(
            &mut stream,
            "HTTP/1.1 200 OK",
            content_for_server.len(),
            &content_for_server,
        )
        .await;
    });

    let dir = unique_test_dir("concurrent-download");
    let dest_path = dir.join("encoder.bin");
    std::fs::create_dir_all(&dir).unwrap();
    let shared = part_path_for(&dest_path);
    std::fs::write(&shared, b"another process is still writing this").unwrap();
    let held = lock_part_file(&shared).expect("stand in for the other process");

    let result = download_file(
        &Client::new(),
        DownloadSpec {
            url: &url,
            filename: "encoder.bin",
            expected_sha256: &sha,
        },
        &dest_path,
        0,
        1,
        &|_: DownloadProgress| {},
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(std::fs::read(&dest_path).unwrap(), content);
    assert_eq!(
        std::fs::read(&shared).unwrap(),
        b"another process is still writing this",
        "the other process's partial file was overwritten"
    );

    drop(held);
    server.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn coordinating_downloaders_never_locks_the_file_the_download_itself_writes() {
    let dir = unique_test_dir("lock-sidecar");
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("encoder-model.int8.onnx");
    let part = part_path_for(&dest);

    let held = lock_part_file(&part).expect("the lock is available");
    assert!(
        !part.exists(),
        "the payload file must stay unlocked and untouched so Windows can still write it"
    );

    std::fs::write(&part, b"the downloader owns this handle").unwrap();
    assert_eq!(
        std::fs::read(&part).unwrap(),
        b"the downloader owns this handle",
        "holding the lock must not block reads or writes of the partial file"
    );

    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}
