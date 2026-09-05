pub mod decoder;
pub mod encoder;
pub mod tokenizer;

use anyhow::{Context, Result};
use ndarray::Array2;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use decoder::TdtDecoder;
use encoder::Encoder;
use tokenizer::Tokenizer;

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
pub struct ModelConfig {
    pub model_type: String,
    pub features_size: usize,
    pub subsampling_factor: usize,
}

/// Complete Parakeet TDT model (encoder + decoder + tokenizer).
pub struct ParakeetModel {
    pub encoder: Encoder,
    pub decoder: TdtDecoder,
    pub tokenizer: Tokenizer,
}

impl ParakeetModel {
    /// Load the complete model from a directory.
    ///
    /// Detects model variant automatically: FP16 > INT8 > FP32.
    pub fn load(
        model_dir: &Path,
        use_accel: bool,
        verbose: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        if verbose {
            println!("Loading Parakeet TDT model from: {}", model_dir.display());
            println!();
        }

        let config_path = model_dir.join("config.json");
        let config: ModelConfig = serde_json::from_str(
            &std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config: {}", config_path.display()))?,
        )
        .context("Failed to parse config.json")?;
        if verbose {
            println!("Model config: {config:?}");
        }

        let has_fp16 = model_dir.join("encoder-model.fp16.onnx").exists();
        let has_int8 = model_dir.join("encoder-model.int8.onnx").exists();
        let has_fp32 = model_dir.join("encoder-model.onnx").exists();

        let (encoder_path, decoder_path, variant) = if has_fp16 {
            (
                model_dir.join("encoder-model.fp16.onnx"),
                model_dir.join("decoder_joint-model.fp16.onnx"),
                "FP16",
            )
        } else if has_int8 {
            (
                model_dir.join("encoder-model.int8.onnx"),
                model_dir.join("decoder_joint-model.int8.onnx"),
                "INT8",
            )
        } else if has_fp32 {
            (
                model_dir.join("encoder-model.onnx"),
                model_dir.join("decoder_joint-model.onnx"),
                "FP32 (legacy)",
            )
        } else {
            anyhow::bail!(
                "No model files found in {}. Run `parakeet download` first.",
                model_dir.display()
            );
        };

        if verbose {
            println!("Using {variant} model");
        }

        let vocab_path = model_dir.join("vocab.txt");
        let tokenizer = Tokenizer::from_file(&vocab_path, verbose)?;
        let vocab_size = tokenizer.vocab_size();

        if cancelled.load(Ordering::Acquire) {
            anyhow::bail!("model load cancelled");
        }

        let accel_cache = if cfg!(target_vendor = "apple") && use_accel {
            Some(model_dir.join("coreml_cache"))
        } else {
            None
        };

        if verbose {
            println!();
        }
        let encoder = Encoder::load(&encoder_path, use_accel, verbose, accel_cache.as_deref())?;

        if cancelled.load(Ordering::Acquire) {
            anyhow::bail!("model load cancelled");
        }

        if verbose {
            println!();
        }
        let decoder = TdtDecoder::load(&decoder_path, vocab_size, verbose)?;

        if verbose {
            println!();
            println!("Model loaded successfully!");
        }

        Ok(Self {
            encoder,
            decoder,
            tokenizer,
        })
    }

    /// Transcribe audio from mel spectrogram features.
    pub fn transcribe(&mut self, features: &Array2<f32>) -> Result<String> {
        let (enc_output, enc_shape, lengths) = self.encoder.encode(features)?;
        let encoded_length = *lengths
            .first()
            .context("Encoder returned no output lengths")?;

        let token_ids = self.decoder.decode_greedy(
            &enc_output,
            &enc_shape,
            encoded_length,
            self.tokenizer.blank_id,
        )?;

        let text = self.tokenizer.decode(&token_ids);

        Ok(text)
    }
}
