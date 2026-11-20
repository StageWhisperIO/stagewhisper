pub const BYTES_1: &[u8] = include_bytes!("../../data/aec_models/model_256_1.onnx");
pub const BYTES_2: &[u8] = include_bytes!("../../data/aec_models/model_256_2.onnx");
pub const STATE_SIZE: usize = 256;
pub const BLOCK_SIZE: usize = 512;
pub const BLOCK_SHIFT: usize = 128;
