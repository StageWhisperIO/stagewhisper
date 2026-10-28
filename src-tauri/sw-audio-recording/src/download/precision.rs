use sha2::{Digest, Sha256};
use std::path::Path;

use crate::vad::file_matches_sha256;

const HF_REPO_FP16: &str = "grikdotnet/parakeet-tdt-0.6b-fp16";
const HF_REPO_FP16_REVISION: &str = "dc9871ec5ad84a420940077e76e8741b3609bf8b";

const HF_REPO_V3: &str = "istupakov/parakeet-tdt-0.6b-v3-onnx";
const HF_REPO_V3_REVISION: &str = "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce";

#[derive(Clone, Copy)]
pub(crate) struct DownloadFile {
    pub(crate) repo: &'static str,
    pub(crate) revision: &'static str,
    pub(crate) filename: &'static str,
    pub(crate) sha256: &'static str,
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

const HF_REPO_110M_EN: &str = "OpenVoiceOS/nvidia-parakeet-tdt_ctc-110m-onnx";
const HF_REPO_110M_EN_REVISION: &str = "40cd0e28e787fc8a2635abf5258e0f886b5304a9";

const PARAKEET_110M_EN_INT8_FILES: &[DownloadFile] = &[
    DownloadFile {
        repo: HF_REPO_110M_EN,
        revision: HF_REPO_110M_EN_REVISION,
        filename: "encoder-model.int8.onnx",
        sha256: "5a7f8c7dcba5b224b967b7b42ce4f1c84e91bc623d0cc76d954c6e0317f1325e",
    },
    DownloadFile {
        repo: HF_REPO_110M_EN,
        revision: HF_REPO_110M_EN_REVISION,
        filename: "decoder_joint-model.int8.onnx",
        sha256: "e1b261a5888667e38d8f700d55ea51171117e14b1e2ee5ebd22f2957dc4fe4c8",
    },
    DownloadFile {
        repo: HF_REPO_110M_EN,
        revision: HF_REPO_110M_EN_REVISION,
        filename: "vocab.txt",
        sha256: "450e56bd2f036fe5b6aa821865838cc5aa9d8b0106134ce9a9ba0664abe6cd10",
    },
    DownloadFile {
        repo: HF_REPO_110M_EN,
        revision: HF_REPO_110M_EN_REVISION,
        filename: "config.json",
        sha256: "78ea780965f996730ac39defe6718ff3afee686d03b363e62ebf512cfa8e4728",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPrecision {
    Fp16,
    Int8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechModel {
    ParakeetMultilingualFp16,
    ParakeetMultilingualInt8,
    ParakeetEnglishInt8,
}

impl SpeechModel {
    pub(crate) fn files(self) -> &'static [DownloadFile] {
        match self {
            SpeechModel::ParakeetMultilingualFp16 => FP16_FILES,
            SpeechModel::ParakeetMultilingualInt8 => INT8_FILES,
            SpeechModel::ParakeetEnglishInt8 => PARAKEET_110M_EN_INT8_FILES,
        }
    }

    pub fn directory_name(self) -> &'static str {
        match self {
            SpeechModel::ParakeetMultilingualFp16 | SpeechModel::ParakeetMultilingualInt8 => {
                "parakeet-tdt-0.6b-v3"
            }
            SpeechModel::ParakeetEnglishInt8 => "parakeet-tdt-ctc-110m-en",
        }
    }

    pub(crate) fn is_mirrored(self) -> bool {
        matches!(self, SpeechModel::ParakeetEnglishInt8)
    }

    pub(crate) fn variant_marker_older_builds_read(self) -> Option<&'static str> {
        match self {
            SpeechModel::ParakeetMultilingualFp16 => Some("FP16"),
            SpeechModel::ParakeetMultilingualInt8 => Some("INT8 quantized"),
            SpeechModel::ParakeetEnglishInt8 => None,
        }
    }
}

impl ModelPrecision {
    pub(crate) fn files(self) -> &'static [DownloadFile] {
        self.model().files()
    }

    pub fn model(self) -> SpeechModel {
        match self {
            ModelPrecision::Fp16 => SpeechModel::ParakeetMultilingualFp16,
            ModelPrecision::Int8 => SpeechModel::ParakeetMultilingualInt8,
        }
    }
}

#[allow(clippy::string_slice)]
pub fn pinned_revision_of(model: SpeechModel) -> String {
    let mut hasher = Sha256::new();
    for file in model.files() {
        hasher.update(file.repo.as_bytes());
        hasher.update(file.revision.as_bytes());
        hasher.update(file.filename.as_bytes());
        hasher.update(file.sha256.as_bytes());
    }
    hasher.update(crate::vad::SILERO_VAD_SHA256.as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

pub fn model_files_present(model_dir: &Path, model: SpeechModel) -> bool {
    model
        .files()
        .iter()
        .all(|dl| model_dir.join(dl.filename).exists())
        && model_dir.join("silero_vad.onnx").exists()
}

pub fn pinned_revision(precision: ModelPrecision) -> String {
    pinned_revision_of(precision.model())
}

fn has_complete_set(model_dir: &Path, precision: ModelPrecision) -> bool {
    precision
        .files()
        .iter()
        .all(|dl| model_dir.join(dl.filename).exists())
        && model_dir.join("silero_vad.onnx").exists()
}

pub fn installed_precision(model_dir: &Path) -> Option<ModelPrecision> {
    if has_complete_set(model_dir, ModelPrecision::Fp16) {
        Some(ModelPrecision::Fp16)
    } else if has_complete_set(model_dir, ModelPrecision::Int8) {
        Some(ModelPrecision::Int8)
    } else {
        None
    }
}

pub fn models_ready(model_dir: &Path) -> bool {
    installed_precision(model_dir).is_some()
}

fn has_complete_verified_set(model_dir: &Path, precision: ModelPrecision) -> bool {
    precision.files().iter().all(|dl| {
        let path = model_dir.join(dl.filename);
        path.exists() && file_matches_sha256(&path, dl.sha256).unwrap_or(false)
    }) && crate::vad::vad_model_ready(model_dir)
}

pub fn models_ready_verified(model_dir: &Path) -> bool {
    has_complete_verified_set(model_dir, ModelPrecision::Fp16)
        || has_complete_verified_set(model_dir, ModelPrecision::Int8)
}

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
