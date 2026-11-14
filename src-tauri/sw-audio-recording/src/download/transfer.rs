use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use super::DownloadProgress;

pub(super) struct DownloadSpec<'a> {
    pub(super) url: &'a str,
    pub(super) filename: &'a str,
    pub(super) expected_sha256: &'a str,
}

pub(super) async fn download_file(
    client: &Client,
    spec: DownloadSpec<'_>,
    dest_path: &Path,
    file_index: usize,
    files_total: usize,
    on_progress: &(impl Fn(DownloadProgress) + Send + 'static),
) -> Result<()> {
    let DownloadSpec {
        url,
        filename,
        expected_sha256,
    } = spec;
    let shared_part_path = part_path_for(dest_path);
    let exclusive_part = lock_part_file(&shared_part_path);
    let part_path = if exclusive_part.is_some() {
        shared_part_path
    } else {
        part_path_private_to_this_process(dest_path)
    };
    let (response, existing_bytes) =
        request_remaining_bytes(client, url, filename, &part_path).await?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to download {filename}: HTTP {}", response.status());
    }

    let resuming = existing_bytes > 0 && response.status() == StatusCode::PARTIAL_CONTENT;

    let (mut file, mut hasher, mut bytes_downloaded) = if resuming {
        let (hasher, hashed_len) = hash_existing_file(&part_path)?;
        let file = OpenOptions::new()
            .append(true)
            .open(&part_path)
            .await
            .with_context(|| {
                format!("Failed to reopen partial download: {}", part_path.display())
            })?;
        (file, hasher, hashed_len)
    } else {
        let file = fs::File::create(&part_path)
            .await
            .with_context(|| format!("Failed to create file: {}", part_path.display()))?;
        (file, Sha256::new(), 0u64)
    };

    let bytes_total = if bytes_downloaded > 0 {
        bytes_downloaded + response.content_length().unwrap_or(0)
    } else {
        response.content_length().unwrap_or(0)
    };

    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("Error downloading {filename}"))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        bytes_downloaded += chunk.len() as u64;

        on_progress(DownloadProgress {
            file_name: filename.to_string(),
            bytes_downloaded,
            bytes_total,
            files_completed: file_index,
            files_total,
        });
    }

    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    let actual_sha256 = format!("{:x}", hasher.finalize());
    if actual_sha256 != expected_sha256 {
        let _ = fs::remove_file(&part_path).await;
        anyhow::bail!(
            "Checksum mismatch for {filename}: expected {expected_sha256}, got {actual_sha256}",
        );
    }

    fs::rename(&part_path, dest_path).await.with_context(|| {
        format!(
            "Failed to rename {} -> {}",
            part_path.display(),
            dest_path.display()
        )
    })?;

    on_progress(DownloadProgress {
        file_name: filename.to_string(),
        bytes_downloaded: bytes_total,
        bytes_total,
        files_completed: file_index + 1,
        files_total,
    });

    Ok(())
}

async fn request_remaining_bytes(
    client: &Client,
    url: &str,
    filename: &str,
    part_path: &Path,
) -> Result<(reqwest::Response, u64)> {
    let existing_bytes = fs::metadata(part_path)
        .await
        .map(|meta| meta.len())
        .unwrap_or(0);
    let response = request_from_offset(client, url, filename, existing_bytes).await?;

    if existing_bytes > 0 && response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = fs::remove_file(part_path).await;
        return Ok((request_from_offset(client, url, filename, 0).await?, 0));
    }

    Ok((response, existing_bytes))
}

async fn request_from_offset(
    client: &Client,
    url: &str,
    filename: &str,
    offset: u64,
) -> Result<reqwest::Response> {
    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
    }
    request
        .send()
        .await
        .with_context(|| format!("Failed to request {filename}"))
}

pub(super) fn part_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let name = tmp
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    tmp.set_file_name(format!(".{name}.part"));
    tmp
}

pub(super) fn part_path_private_to_this_process(path: &Path) -> PathBuf {
    let shared = part_path_for(path);
    let mut private = shared.clone();
    let name = shared
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    private.set_file_name(format!("{name}.{}", std::process::id()));
    private
}

fn lock_path_for(part_path: &Path) -> PathBuf {
    let mut lock = part_path.to_path_buf();
    let name = part_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    lock.set_file_name(format!("{name}.lock"));
    lock
}

pub(super) fn lock_part_file(part_path: &Path) -> Option<std::fs::File> {
    let lock_path = lock_path_for(part_path);
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

fn hash_existing_file(path: &Path) -> Result<(Sha256, u64)> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open partial download: {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;

    loop {
        let read = reader
            .read(&mut buf)
            .with_context(|| format!("Failed to read partial download: {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        total += read as u64;
    }

    Ok((hasher, total))
}
