mod precision;
mod transfer;

#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use reqwest::Client;
use std::path::{Path, PathBuf};
use tokio::fs;

pub use precision::{
    installed_precision, model_exists, model_files_present, models_ready, models_ready_verified,
    pinned_revision, pinned_revision_of, ModelPrecision, SpeechModel,
};

use crate::vad::file_matches_sha256;
use precision::DownloadFile;
use transfer::{download_file, DownloadSpec};

const HF_BASE_URL: &str = "https://huggingface.co";
const MIRROR_BASE_URL: &str = "https://dl.stagewhisper.io/models";
const USER_AGENT: &str = "stagewhisper-desktop/0.1.0";
const VARIANT_MARKER: &str = ".variant";
const VERIFIED_MARKER: &str = ".verified";
const VERIFIED_MARKER_CONTENTS: &str = "sha256-ok";

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

fn resolve_url(dl: &DownloadFile) -> String {
    format!(
        "{HF_BASE_URL}/{}/resolve/{}/{}",
        dl.repo, dl.revision, dl.filename
    )
}

fn mirror_url(model: SpeechModel, dl: &DownloadFile) -> Option<String> {
    model.is_mirrored().then(|| {
        format!(
            "{MIRROR_BASE_URL}/{}/{}",
            model.directory_name(),
            dl.filename
        )
    })
}

pub async fn download_model(
    model_dir: &Path,
    precision: ModelPrecision,
    on_progress: impl Fn(DownloadProgress) + Send + 'static,
) -> Result<()> {
    download_speech_model(model_dir, precision.model(), on_progress).await
}

pub async fn download_speech_model(
    model_dir: &Path,
    model: SpeechModel,
    on_progress: impl Fn(DownloadProgress) + Send + 'static,
) -> Result<()> {
    let files = model.files();
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

        let from_origin = download_file(
            &client,
            DownloadSpec {
                url: &resolve_url(dl),
                filename: dl.filename,
                expected_sha256: dl.sha256,
            },
            &dest_path,
            index,
            files_total,
            &on_progress,
        )
        .await;

        match (from_origin, mirror_url(model, dl)) {
            (Ok(()), _) => {}
            (Err(origin_error), Some(mirror)) => {
                let _ = fs::remove_file(transfer::part_path_for(&dest_path)).await;
                download_file(
                    &client,
                    DownloadSpec {
                        url: &mirror,
                        filename: dl.filename,
                        expected_sha256: dl.sha256,
                    },
                    &dest_path,
                    index,
                    files_total,
                    &on_progress,
                )
                .await
                .with_context(|| {
                    format!("Mirror also failed after origin error: {origin_error}")
                })?;
            }
            (Err(origin_error), None) => return Err(origin_error),
        }
    }

    if let Some(variant) = model.variant_marker_older_builds_read() {
        fs::write(model_dir.join(VARIANT_MARKER), variant).await?;
        fs::write(model_dir.join(VERIFIED_MARKER), VERIFIED_MARKER_CONTENTS).await?;
    }

    Ok(())
}

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

        let url = format!(
            "{HF_BASE_URL}/{}/resolve/{}/{}",
            dl.repo, dl.revision, remote_filename
        );

        download_file(
            &client,
            DownloadSpec {
                url: &url,
                filename: dl.filename,
                expected_sha256: dl.sha256,
            },
            &dest_path,
            index,
            files_total,
            &on_progress,
        )
        .await?;
    }

    Ok(())
}
