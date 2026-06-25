use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const HF_BASE_URL: &str = "https://huggingface.co";
const USER_AGENT: &str = "stagewhisper-desktop/0.1.0";

const HF_REPO_FP16: &str = "grikdotnet/parakeet-tdt-0.6b-fp16";
const HF_REPO_FP16_REVISION: &str = "dc9871ec5ad84a420940077e76e8741b3609bf8b";

const HF_REPO_V3: &str = "istupakov/parakeet-tdt-0.6b-v3-onnx";
const HF_REPO_V3_REVISION: &str = "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce";

struct DownloadFile {
    repo: &'static str,
    revision: &'static str,
    filename: &'static str,
    sha256: &'static str,
}

const FP16_FILES: &[DownloadFile] = &[
    DownloadFile {
        repo: HF_REPO_FP16,
        revision: HF_REPO_FP16_REVISION,
        filename: "encoder-model.fp16.onnx",
        sha256: "a2bdeeb99cb7e5548818e823127b33854dd0c26f5d0c8da91effdd895ea0e717",
    },
    DownloadFile {
        repo: HF_REPO_FP16,
        revision: HF_REPO_FP16_REVISION,
        filename: "decoder_joint-model.fp16.onnx",
        sha256: "b33a73b7c1d71b9d5a0911f5cb478be3dcbf79f53355c531ab1cd1dcd68ad8ef",
    },
    DownloadFile {
        repo: HF_REPO_V3,
        revision: HF_REPO_V3_REVISION,
        filename: "vocab.txt",
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    },
    DownloadFile {
        repo: HF_REPO_V3,
        revision: HF_REPO_V3_REVISION,
        filename: "config.json",
        sha256: "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466",
    },
];

const INT8_FILES: &[DownloadFile] = &[
    DownloadFile {
        repo: HF_REPO_V3,
        revision: HF_REPO_V3_REVISION,
        filename: "encoder-model.int8.onnx",
        sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
    },
    DownloadFile {
        repo: HF_REPO_V3,
        revision: HF_REPO_V3_REVISION,
        filename: "decoder_joint-model.int8.onnx",
        sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
    },
    DownloadFile {
        repo: HF_REPO_V3,
        revision: HF_REPO_V3_REVISION,
        filename: "vocab.txt",
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    },
    DownloadFile {
        repo: HF_REPO_V3,
        revision: HF_REPO_V3_REVISION,
        filename: "config.json",
        sha256: "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466",
    },
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub file_name: String,
    pub bytes_downloaded: u64,
    pub bytes_total: u64,
    pub files_completed: usize,
    pub files_total: usize,
}

pub fn default_model_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.stagewhisper.app")
        .join("models")
        .join("parakeet-tdt-0.6b-v3")
}

pub fn models_ready(model_dir: &Path) -> bool {
    let has_files = || {
        FP16_FILES
            .iter()
            .all(|dl| model_dir.join(dl.filename).exists())
            && model_dir.join("silero_vad.onnx").exists()
    };

    let verified_marker = model_dir.join(".verified");
    if verified_marker.exists() {
        return has_files();
    }

    if model_dir.join(".variant").exists() && has_files() {
        let _ = std::fs::write(&verified_marker, "sha256-ok");
        return true;
    }

    false
}

/// Full SHA-256 verification of all model files. Expensive — only call during/after download.
pub fn models_ready_verified(model_dir: &Path) -> bool {
    let parakeet_ready = FP16_FILES.iter().all(|dl| {
        let path = model_dir.join(dl.filename);
        path.exists() && file_matches_sha256(&path, dl.sha256).unwrap_or(false)
    });
    parakeet_ready && crate::vad::vad_model_ready(model_dir)
}

/// Check if model files exist (any variant).
pub fn model_exists(model_dir: &Path) -> bool {
    let has_fp16 = model_dir.join("encoder-model.fp16.onnx").exists()
        && model_dir.join("decoder_joint-model.fp16.onnx").exists();
    let has_int8 = model_dir.join("encoder-model.int8.onnx").exists()
        && model_dir.join("decoder_joint-model.int8.onnx").exists();
    let has_fp32 = model_dir.join("encoder-model.onnx").exists()
        && model_dir.join("decoder_joint-model.onnx").exists();

    (has_fp16 || has_int8 || has_fp32)
        && model_dir.join("vocab.txt").exists()
        && model_dir.join("config.json").exists()
}

/// Download model files with progress reporting via callback.
pub async fn download_model(
    model_dir: &Path,
    int8: bool,
    on_progress: impl Fn(DownloadProgress) + Send + 'static,
) -> Result<()> {
    let files = if int8 { INT8_FILES } else { FP16_FILES };
    let variant = if int8 { "INT8 quantized" } else { "FP16" };
    let files_total = files.len();

    fs::create_dir_all(model_dir)
        .await
        .with_context(|| format!("Failed to create directory: {}", model_dir.display()))?;

    let client = Client::builder()
        .https_only(true)
        .user_agent(USER_AGENT)
        .build()?;

    for (index, dl) in files.iter().enumerate() {
        let dest_path = model_dir.join(dl.filename);

        if dest_path.exists() {
            if file_matches_sha256(&dest_path, dl.sha256)? {
                on_progress(DownloadProgress {
                    file_name: dl.filename.to_string(),
                    bytes_downloaded: 0,
                    bytes_total: 0,
                    files_completed: index + 1,
                    files_total,
                });
                continue;
            }

            fs::remove_file(&dest_path).await.with_context(|| {
                format!(
                    "Failed to remove invalid cached file before re-download: {}",
                    dest_path.display()
                )
            })?;
        }

        download_file(&client, dl, &dest_path, index, files_total, &on_progress).await?;
    }

    let marker = model_dir.join(".variant");
    fs::write(&marker, variant).await?;

    let verified_marker = model_dir.join(".verified");
    fs::write(&verified_marker, "sha256-ok").await?;

    Ok(())
}

async fn download_file(
    client: &Client,
    spec: &DownloadFile,
    dest_path: &Path,
    file_index: usize,
    files_total: usize,
    on_progress: &(impl Fn(DownloadProgress) + Send + 'static),
) -> Result<()> {
    let url = format!(
        "{HF_BASE_URL}/{}/resolve/{}/{}",
        spec.repo, spec.revision, spec.filename
    );

    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Failed to request {}", spec.filename))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to download {}: HTTP {}",
            spec.filename,
            response.status()
        );
    }

    let bytes_total = response.content_length().unwrap_or(0);

    let tmp_path = temp_path(dest_path);
    let _ = fs::remove_file(&tmp_path).await;
    let mut file = fs::File::create(&tmp_path)
        .await
        .with_context(|| format!("Failed to create file: {}", tmp_path.display()))?;

    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    let mut bytes_downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("Error downloading {}", spec.filename))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        bytes_downloaded += chunk.len() as u64;

        on_progress(DownloadProgress {
            file_name: spec.filename.to_string(),
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
    if actual_sha256 != spec.sha256 {
        let _ = fs::remove_file(&tmp_path).await;
        anyhow::bail!(
            "Checksum mismatch for {}: expected {}, got {}",
            spec.filename,
            spec.sha256,
            actual_sha256,
        );
    }

    fs::rename(&tmp_path, dest_path).await.with_context(|| {
        format!(
            "Failed to rename {} -> {}",
            tmp_path.display(),
            dest_path.display()
        )
    })?;

    on_progress(DownloadProgress {
        file_name: spec.filename.to_string(),
        bytes_downloaded: bytes_total,
        bytes_total,
        files_completed: file_index + 1,
        files_total,
    });

    Ok(())
}

use crate::vad::file_matches_sha256;

const HF_REPO_SPEAKER: &str = "csukuangfj/speaker-embedding-models";
const HF_REPO_SPEAKER_REVISION: &str = "0743f301363dec56491a490f6d6cbc9d67f9a3bf";
const HF_REPO_SEGMENTATION: &str = "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0";
const HF_REPO_SEGMENTATION_REVISION: &str = "9403a6902bb58e3d5ae8c7e77c3422de279db2e0";

pub const SPEAKER_EMBEDDING_FILENAME: &str = "nemo_en_titanet_small.onnx";
pub const SPEAKER_SEGMENTATION_FILENAME: &str = "pyannote_segmentation_3_0.int8.onnx";

const SPEAKER_FILES: &[DownloadFile] = &[
    DownloadFile {
        repo: HF_REPO_SPEAKER,
        revision: HF_REPO_SPEAKER_REVISION,
        filename: SPEAKER_EMBEDDING_FILENAME,
        sha256: "ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e",
    },
    DownloadFile {
        repo: HF_REPO_SEGMENTATION,
        revision: HF_REPO_SEGMENTATION_REVISION,
        filename: SPEAKER_SEGMENTATION_FILENAME,
        sha256: "d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d",
    },
];

const SPEAKER_SEGMENTATION_REMOTE_FILENAME: &str = "model.int8.onnx";

pub fn speaker_embedding_path(model_dir: &Path) -> PathBuf {
    model_dir.join(SPEAKER_EMBEDDING_FILENAME)
}

pub fn speaker_segmentation_path(model_dir: &Path) -> PathBuf {
    model_dir.join(SPEAKER_SEGMENTATION_FILENAME)
}

pub fn speaker_embedding_ready(model_dir: &Path) -> bool {
    let path = speaker_embedding_path(model_dir);
    path.exists() && file_matches_sha256(&path, SPEAKER_FILES[0].sha256).unwrap_or(false)
}

pub fn speaker_models_ready(model_dir: &Path) -> bool {
    SPEAKER_FILES.iter().all(|dl| {
        let path = model_dir.join(dl.filename);
        path.exists() && file_matches_sha256(&path, dl.sha256).unwrap_or(false)
    })
}

pub async fn download_speaker_models(
    model_dir: &Path,
    on_progress: impl Fn(DownloadProgress) + Send + 'static,
) -> Result<()> {
    fs::create_dir_all(model_dir)
        .await
        .with_context(|| format!("Failed to create directory: {}", model_dir.display()))?;

    let client = Client::builder()
        .https_only(true)
        .user_agent(USER_AGENT)
        .build()?;

    let files_total = SPEAKER_FILES.len();

    for (index, dl) in SPEAKER_FILES.iter().enumerate() {
        let dest_path = model_dir.join(dl.filename);

        if dest_path.exists() {
            if file_matches_sha256(&dest_path, dl.sha256)? {
                on_progress(DownloadProgress {
                    file_name: dl.filename.to_string(),
                    bytes_downloaded: 0,
                    bytes_total: 0,
                    files_completed: index + 1,
                    files_total,
                });
                continue;
            }
            fs::remove_file(&dest_path).await.with_context(|| {
                format!(
                    "Failed to remove invalid cached file before re-download: {}",
                    dest_path.display()
                )
            })?;
        }

        let remote_filename = if dl.filename == SPEAKER_SEGMENTATION_FILENAME {
            SPEAKER_SEGMENTATION_REMOTE_FILENAME
        } else {
            dl.filename
        };

        let spec = DownloadFile {
            repo: dl.repo,
            revision: dl.revision,
            filename: remote_filename,
            sha256: dl.sha256,
        };

        download_file(&client, &spec, &dest_path, index, files_total, &on_progress).await?;
    }

    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let name = tmp
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    tmp.set_file_name(format!(".{name}.tmp"));
    tmp
}
