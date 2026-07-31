use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

/// TDT (Token-and-Duration Transducer) decoder for Parakeet.
pub struct TdtDecoder {
    session: Session,
    vocab_size: usize,
    num_durations: usize,
    lstm_hidden: usize,
}

impl TdtDecoder {
    /// Load the decoder_joint ONNX model (CPU only).
    pub fn load(path: &Path, vocab_size: usize, verbose: bool) -> Result<Self> {
        let mut builder = Session::builder().map_err(|e| anyhow::anyhow!("{e}"))?;
        let session = builder
            .commit_from_file(path)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("Failed to load decoder model: {}", path.display()))?;

        if verbose {
            println!("Decoder loaded (vocab_size={vocab_size}):");
            for input in session.inputs() {
                println!("  input: {} {:?}", input.name(), input.dtype());
            }
            for output in session.outputs() {
                println!("  output: {} {:?}", output.name(), output.dtype());
            }
        }

        Ok(Self {
            session,
            vocab_size,
            num_durations: 5,
            lstm_hidden: 640,
        })
    }

    /// Run greedy TDT decoding on encoder output.
    pub fn decode_greedy(
        &mut self,
        encoder_output: &[f32],
        enc_shape: &[usize],
        encoded_length: i64,
        blank_id: usize,
    ) -> Result<Vec<usize>> {
        if enc_shape.len() != 3 {
            anyhow::bail!(
                "Encoder returned invalid shape {:?}; expected [batch, hidden_dim, time]",
                enc_shape
            );
        }
        if encoded_length < 0 {
            anyhow::bail!("Encoder returned negative encoded length: {encoded_length}");
        }

        let _batch = enc_shape[0];
        let hidden_dim = enc_shape[1];
        let time_dim = enc_shape[2];

        let expected_len = hidden_dim
            .checked_mul(time_dim)
            .context("Encoder output shape overflowed while validating dimensions")?;
        if encoder_output.len() < expected_len {
            anyhow::bail!(
                "Encoder output buffer too small: got {}, expected at least {} for shape {:?}",
                encoder_output.len(),
                expected_len,
                enc_shape,
            );
        }

        let max_steps = (encoded_length as usize).min(time_dim);

        let mut tokens: Vec<usize> = Vec::new();
        let mut position: usize = 0;

        let mut last_label: i32 = blank_id as i32;

        let mut state1 = vec![0.0f32; 2 * self.lstm_hidden];
        let mut state2 = vec![0.0f32; 2 * self.lstm_hidden];

        let max_iterations = max_steps * 10;
        let mut iterations = 0;

        while position < max_steps && iterations < max_iterations {
            iterations += 1;

            let mut enc_slice = vec![0.0f32; hidden_dim];
            for h in 0..hidden_dim {
                enc_slice[h] = encoder_output[h * time_dim + position];
            }

            let enc_tensor = Tensor::from_array(([1usize, hidden_dim, 1], enc_slice))
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let targets = Tensor::from_array(([1usize, 1], vec![last_label]))
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let target_length =
                Tensor::from_array(([1usize], vec![1i32])).map_err(|e| anyhow::anyhow!("{e}"))?;

            let states1_tensor =
                Tensor::from_array(([2usize, 1, self.lstm_hidden], state1.clone()))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;

            let states2_tensor =
                Tensor::from_array(([2usize, 1, self.lstm_hidden], state2.clone()))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;

            let outputs = self
                .session
                .run(ort::inputs![
                    "encoder_outputs" => enc_tensor,
                    "targets" => targets,
                    "target_length" => target_length,
                    "input_states_1" => states1_tensor,
                    "input_states_2" => states2_tensor,
                ])
                .map_err(|e| anyhow::anyhow!("{e}"))
                .context("Decoder inference failed")?;

            let (_logits_shape, logits_data) = outputs["outputs"]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("{e}"))
                .context("Failed to extract logits")?;

            let total = self.vocab_size + self.num_durations;
            if logits_data.len() < total {
                anyhow::bail!(
                    "Decoder logits too small: got {}, expected at least {total}",
                    logits_data.len(),
                );
            }
            let offset = logits_data.len() - total;
            let token_logits = &logits_data[offset..offset + self.vocab_size];
            let duration_logits = &logits_data[offset + self.vocab_size..];

            let (token_id, _) = argmax(token_logits);
            let (duration, _) = argmax(duration_logits);

            if token_id == blank_id {
                position += duration.max(1);
            } else {
                tokens.push(token_id);
                last_label = token_id as i32;

                let (_s1_shape, s1_data) = outputs["output_states_1"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                state1 = s1_data.to_vec();

                let (_s2_shape, s2_data) = outputs["output_states_2"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                state2 = s2_data.to_vec();

                position += duration.max(1);
            }
        }

        Ok(tokens)
    }
}

pub(crate) fn argmax(slice: &[f32]) -> (usize, f32) {
    let mut max_idx = 0;
    let mut max_val = f32::NEG_INFINITY;
    for (i, &v) in slice.iter().enumerate() {
        if v > max_val {
            max_val = v;
            max_idx = i;
        }
    }
    (max_idx, max_val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argmax_basic() {
        let slice = [1.0f32, 3.0, 2.0, 0.5];
        let (idx, val) = argmax(&slice);
        assert_eq!(idx, 1);
        assert_eq!(val, 3.0);
    }

    #[test]
    fn test_argmax_single_element() {
        let slice = [42.0f32];
        let (idx, val) = argmax(&slice);
        assert_eq!(idx, 0);
        assert_eq!(val, 42.0);
    }

    #[test]
    fn test_argmax_negative_values() {
        let slice = [-5.0f32, -2.0, -8.0, -1.0, -3.0];
        let (idx, val) = argmax(&slice);
        assert_eq!(idx, 3);
        assert_eq!(val, -1.0);
    }

    #[test]
    fn test_argmax_ties_first_wins() {
        let slice = [1.0f32, 5.0, 5.0, 3.0];
        let (idx, val) = argmax(&slice);
        assert_eq!(idx, 1);
        assert_eq!(val, 5.0);
    }
}
