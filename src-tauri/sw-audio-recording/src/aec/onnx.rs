use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::TensorRef;
use realfft::{num_complex::Complex, ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

use super::model;
use super::{AecResult, CircularBuffer};

fn load_session(bytes: &[u8]) -> AecResult<Session> {
    Ok(Session::builder()?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .commit_from_memory(bytes)?)
}

struct ProcessingContext {
    scratch: Vec<Complex<f32>>,
    ifft_scratch: Vec<Complex<f32>>,
    in_buffer_fft: Vec<f32>,
    in_block_fft: Vec<Complex<f32>>,
    lpb_buffer_fft: Vec<f32>,
    lpb_block_fft: Vec<Complex<f32>>,
    estimated_block_vec: Vec<f32>,
    in_mag: Vec<f32>,
    lpb_mag: Vec<f32>,
    estimated_block: Vec<f32>,
    in_lpb: Vec<f32>,
    out_mask: Vec<f32>,
    out_block: Vec<f32>,
}

impl ProcessingContext {
    fn new(
        block_len: usize,
        fft: &Arc<dyn RealToComplex<f32>>,
        ifft: &Arc<dyn ComplexToReal<f32>>,
    ) -> Self {
        let spectrum_len = block_len / 2 + 1;
        Self {
            scratch: vec![Complex::new(0.0f32, 0.0f32); fft.get_scratch_len()],
            ifft_scratch: vec![Complex::new(0.0f32, 0.0f32); ifft.get_scratch_len()],
            in_buffer_fft: vec![0.0f32; block_len],
            in_block_fft: vec![Complex::new(0.0f32, 0.0f32); spectrum_len],
            lpb_buffer_fft: vec![0.0f32; block_len],
            lpb_block_fft: vec![Complex::new(0.0f32, 0.0f32); spectrum_len],
            estimated_block_vec: vec![0.0f32; block_len],
            in_mag: vec![0.0f32; spectrum_len],
            lpb_mag: vec![0.0f32; spectrum_len],
            estimated_block: vec![0.0f32; block_len],
            in_lpb: vec![0.0f32; block_len],
            out_mask: vec![0.0f32; spectrum_len],
            out_block: vec![0.0f32; block_len],
        }
    }
}

pub struct Aec {
    session_1: Session,
    session_2: Session,
    block_len: usize,
    block_shift: usize,
    spectrum_len: usize,
    state_size: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    states_1: Vec<f32>,
    states_2: Vec<f32>,
    in_buffer: CircularBuffer,
    in_buffer_lpb: CircularBuffer,
    out_buffer: CircularBuffer,
}

impl Aec {
    pub fn new() -> AecResult<Self> {
        let (block_len, block_shift) = (model::BLOCK_SIZE, model::BLOCK_SHIFT);

        let mut fft_planner = RealFftPlanner::<f32>::new();
        let fft = fft_planner.plan_fft_forward(block_len);
        let ifft = fft_planner.plan_fft_inverse(block_len);

        let session_1 = load_session(model::BYTES_1)?;
        let session_2 = load_session(model::BYTES_2)?;

        let state_size = model::STATE_SIZE;

        Ok(Aec {
            session_1,
            session_2,
            block_len,
            block_shift,
            spectrum_len: block_len / 2 + 1,
            state_size,
            fft,
            ifft,
            states_1: vec![0.0f32; 2 * state_size * 2],
            states_2: vec![0.0f32; 2 * state_size * 2],
            in_buffer: CircularBuffer::new(block_len, block_shift),
            in_buffer_lpb: CircularBuffer::new(block_len, block_shift),
            out_buffer: CircularBuffer::new(block_len, block_shift),
        })
    }

    fn calculate_fft_magnitude(
        &self,
        input: &[f32],
        fft_buffer: &mut [f32],
        fft_result: &mut [Complex<f32>],
        scratch: &mut [Complex<f32>],
        magnitude: &mut [f32],
    ) -> AecResult<()> {
        fft_buffer.copy_from_slice(input);
        self.fft
            .process_with_scratch(fft_buffer, fft_result, scratch)?;

        for (m, c) in magnitude.iter_mut().zip(fft_result.iter()) {
            *m = c.norm();
        }

        Ok(())
    }

    fn run_model_1(&mut self, ctx: &mut ProcessingContext) -> AecResult<()> {
        let mag_shape = [1usize, 1, self.spectrum_len];
        let state_shape = [1usize, 2, self.state_size, 2];
        let mut outputs = self.session_1.run(ort::inputs![
            TensorRef::from_array_view((mag_shape, ctx.in_mag.as_slice()))?,
            TensorRef::from_array_view((state_shape, self.states_1.as_slice()))?,
            TensorRef::from_array_view((mag_shape, ctx.lpb_mag.as_slice()))?
        ])?;

        let out_mask = outputs
            .remove("Identity")
            .ok_or("missing AEC output tensor: Identity")?;
        let (_, mask) = out_mask.try_extract_tensor::<f32>()?;
        ctx.out_mask.copy_from_slice(mask);

        let new_states = outputs
            .remove("Identity_1")
            .ok_or("missing AEC output tensor: Identity_1")?;
        let (_, states) = new_states.try_extract_tensor::<f32>()?;
        self.states_1.copy_from_slice(states);

        Ok(())
    }

    fn run_model_2(&mut self, ctx: &mut ProcessingContext) -> AecResult<()> {
        let block_shape = [1usize, 1, self.block_len];
        let state_shape = [1usize, 2, self.state_size, 2];
        let mut outputs = self.session_2.run(ort::inputs![
            TensorRef::from_array_view((block_shape, ctx.estimated_block.as_slice()))?,
            TensorRef::from_array_view((state_shape, self.states_2.as_slice()))?,
            TensorRef::from_array_view((block_shape, ctx.in_lpb.as_slice()))?
        ])?;

        let out_block = outputs
            .remove("Identity")
            .ok_or("missing AEC output tensor: Identity")?;
        let (_, block) = out_block.try_extract_tensor::<f32>()?;
        ctx.out_block.copy_from_slice(block);

        let new_states = outputs
            .remove("Identity_1")
            .ok_or("missing AEC output tensor: Identity_1")?;
        let (_, states) = new_states.try_extract_tensor::<f32>()?;
        self.states_2.copy_from_slice(states);

        Ok(())
    }

    pub fn process_streaming(&mut self, mic_input: &[f32], lpb_input: &[f32]) -> AecResult<Vec<f32>> {
        let len_audio = mic_input.len().min(lpb_input.len());
        if len_audio == 0 {
            return Ok(vec![]);
        }
        let mic_input = &mic_input[..len_audio];
        let lpb_input = &lpb_input[..len_audio];

        let mut out_file = vec![0.0f32; len_audio];
        let num_blocks = len_audio / self.block_shift;
        let mut ctx = ProcessingContext::new(self.block_len, &self.fft, &self.ifft);

        for idx in 0..num_blocks {
            let start = idx * self.block_shift;
            let end = (start + self.block_shift).min(len_audio);

            self.in_buffer.push_chunk(&mic_input[start..end]);
            self.in_buffer_lpb.push_chunk(&lpb_input[start..end]);

            self.calculate_fft_magnitude(
                self.in_buffer.data(),
                &mut ctx.in_buffer_fft,
                &mut ctx.in_block_fft,
                &mut ctx.scratch,
                &mut ctx.in_mag,
            )?;

            self.calculate_fft_magnitude(
                self.in_buffer_lpb.data(),
                &mut ctx.lpb_buffer_fft,
                &mut ctx.lpb_block_fft,
                &mut ctx.scratch,
                &mut ctx.lpb_mag,
            )?;

            self.run_model_1(&mut ctx)?;

            for (c, &m) in ctx.in_block_fft.iter_mut().zip(ctx.out_mask.iter()) {
                *c *= m;
            }

            self.ifft.process_with_scratch(
                &mut ctx.in_block_fft,
                &mut ctx.estimated_block_vec,
                &mut ctx.ifft_scratch,
            )?;

            let norm_factor = 1.0 / self.block_len as f32;
            for (d, &s) in ctx.estimated_block.iter_mut().zip(ctx.estimated_block_vec.iter()) {
                *d = s * norm_factor;
            }
            ctx.in_lpb.copy_from_slice(self.in_buffer_lpb.data());

            self.run_model_2(&mut ctx)?;

            self.out_buffer.shift_and_accumulate(&ctx.out_block);

            let out_start = idx * self.block_shift;
            let out_end = (out_start + self.block_shift).min(out_file.len());
            let out_chunk_len = out_end - out_start;
            if out_chunk_len > 0 {
                out_file[out_start..out_end]
                    .copy_from_slice(&self.out_buffer.data()[..out_chunk_len]);
            }
        }

        Ok(out_file)
    }
}
