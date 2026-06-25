use sw_audio_recording::aec::Aec;

fn read_wav(path: &str) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mic = read_wav(&args[1]);
    let lpb = read_wav(&args[2]);
    let n = mic.len().min(lpb.len());

    let mut aec = Aec::new().expect("failed to build AEC");
    let chunk = 512 * 2;
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let end = (i + chunk).min(n);
        let cleaned = aec
            .process_streaming(&mic[i..end], &lpb[i..end])
            .expect("process_streaming failed");
        out.extend(cleaned);
        i = end;
    }

    let finite = out.iter().all(|v| v.is_finite());
    let rms_mic = rms(&mic[..n]);
    let rms_out = rms(&out);
    let reduction_db = 20.0 * (rms_out.max(1e-9) / rms_mic.max(1e-9)).log10();
    println!(
        "[aec_probe] in={} out={} finite={} rms_mic={:.5} rms_out={:.5} reduction_dB={:.2}",
        n, out.len(), finite, rms_mic, rms_out, reduction_db
    );

    if args.len() > 3 {
        let near = read_wav(&args[3]);
        let err_at = |delay: usize| -> f32 {
            let mut acc = 0.0f32;
            let mut count = 0usize;
            for j in 0..n {
                if j + delay >= out.len() || j >= near.len() {
                    break;
                }
                let d = out[j + delay] - near[j];
                acc += d * d;
                count += 1;
            }
            if count == 0 {
                f32::INFINITY
            } else {
                (acc / count as f32).sqrt()
            }
        };
        let err_in = rms(
            &(0..n.min(near.len()))
                .map(|j| mic[j] - near[j])
                .collect::<Vec<f32>>(),
        );
        let mut best_err_out = f32::INFINITY;
        let mut best_delay = 0;
        for delay in 0..640 {
            let e = err_at(delay);
            if e < best_err_out {
                best_err_out = e;
                best_delay = delay;
            }
        }
        let improvement_db = 20.0 * (best_err_out.max(1e-9) / err_in.max(1e-9)).log10();
        println!(
            "[aec_probe] vs clean near: err_in={:.5} err_out={:.5} (delay={}) improvement_dB={:.2}",
            err_in, best_err_out, best_delay, improvement_db
        );
    }
}
