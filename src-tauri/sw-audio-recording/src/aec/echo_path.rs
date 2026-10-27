use std::collections::VecDeque;

pub const FAR_ACTIVE_RMS: f32 = 1e-3;

const ENV_CAPACITY: usize = 128;
const ANALYSIS_WINDOW: usize = 96;
const ANALYZE_EVERY: u64 = 32;
const MAX_LAG_BLOCKS: usize = 32;
const MIN_VALID_BLOCKS: usize = 64;
const MIN_FAR_ACTIVE_BLOCKS: usize = 24;
const MIN_CORR_PAIRS: usize = 24;
const MIN_FAR_MODULATION: f32 = 0.25;
const CORR_ECHO: f32 = 0.60;
const CORR_UNRELATED: f32 = 0.30;
const NEAR_ACTIVE_FLOOR_MULT: f32 = 3.0;
const NEAR_QUIET_FRAC: f32 = 0.10;
const NEAR_ACTIVE_FRAC: f32 = 0.20;
const EVIDENCE_DECAY: f32 = 0.9;
const ENGAGE_AT: f32 = 2.0;
const BYPASS_AT: f32 = 12.0;
const FLOOR_RISE_PER_BLOCK: f32 = 1.005;
const FLOOR_MIN: f32 = 1e-5;
const EPS: f32 = 1e-9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EchoPathPrior {
    EchoProne,
    Clear,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EchoRegime {
    Engaged,
    Bypassed,
}

impl EchoPathPrior {
    fn initial_regime(self) -> EchoRegime {
        match self {
            EchoPathPrior::Clear => EchoRegime::Bypassed,
            EchoPathPrior::EchoProne | EchoPathPrior::Unknown => EchoRegime::Engaged,
        }
    }
}

pub struct EchoPathDetector {
    near_env: VecDeque<f32>,
    far_env: VecDeque<Option<f32>>,
    blocks: u64,
    near_floor: f32,
    engage_evidence: f32,
    bypass_evidence: f32,
    regime: EchoRegime,
}

impl EchoPathDetector {
    pub fn new(prior: EchoPathPrior) -> Self {
        Self {
            near_env: VecDeque::with_capacity(ENV_CAPACITY),
            far_env: VecDeque::with_capacity(ENV_CAPACITY),
            blocks: 0,
            near_floor: f32::MAX,
            engage_evidence: 0.0,
            bypass_evidence: 0.0,
            regime: prior.initial_regime(),
        }
    }

    pub fn reset(&mut self, prior: EchoPathPrior) {
        *self = Self::new(prior);
    }

    pub fn regime(&self) -> EchoRegime {
        self.regime
    }

    pub fn push_block(&mut self, near_rms: f32, far_rms: Option<f32>) {
        self.near_floor = if near_rms < self.near_floor {
            near_rms.max(FLOOR_MIN)
        } else {
            (self.near_floor * FLOOR_RISE_PER_BLOCK).min(f32::MAX / 2.0)
        };

        if self.near_env.len() == ENV_CAPACITY {
            self.near_env.pop_front();
            self.far_env.pop_front();
        }
        self.near_env.push_back(near_rms);
        self.far_env.push_back(far_rms);

        self.blocks += 1;
        if self.blocks % ANALYZE_EVERY == 0 && self.near_env.len() >= ANALYSIS_WINDOW {
            self.analyze();
        }
    }

    fn analyze(&mut self) {
        let start = self.near_env.len() - ANALYSIS_WINDOW;
        let near: Vec<f32> = self.near_env.iter().skip(start).copied().collect();
        let far: Vec<Option<f32>> = self.far_env.iter().skip(start).copied().collect();

        let valid: Vec<f32> = far.iter().filter_map(|v| *v).collect();
        if valid.len() < MIN_VALID_BLOCKS {
            return;
        }

        let active_blocks: Vec<usize> = far
            .iter()
            .enumerate()
            .filter(|(_, v)| v.map_or(false, |r| r >= FAR_ACTIVE_RMS))
            .map(|(i, _)| i)
            .collect();
        if active_blocks.len() < MIN_FAR_ACTIVE_BLOCKS {
            return;
        }

        let mean = valid.iter().sum::<f32>() / valid.len() as f32;
        let var = valid.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / valid.len() as f32;
        if var.sqrt() / (mean + EPS) < MIN_FAR_MODULATION {
            return;
        }

        let corr = (0..=MAX_LAG_BLOCKS)
            .filter_map(|lag| lagged_envelope_corr(&near, &far, lag))
            .fold(0.0f32, f32::max);

        let active_threshold = NEAR_ACTIVE_FLOOR_MULT * self.near_floor;
        let near_active = active_blocks
            .iter()
            .filter(|&&i| near[i] > active_threshold)
            .count();
        let near_active_frac = near_active as f32 / active_blocks.len() as f32;

        if corr >= CORR_ECHO && near_active_frac >= NEAR_ACTIVE_FRAC {
            self.engage_evidence += 1.0;
            self.bypass_evidence = 0.0;
        } else if near_active_frac <= NEAR_QUIET_FRAC {
            self.bypass_evidence += 1.0;
            self.engage_evidence *= 0.5;
        } else if corr <= CORR_UNRELATED {
            self.engage_evidence *= EVIDENCE_DECAY;
            self.bypass_evidence *= EVIDENCE_DECAY;
        } else {
            self.engage_evidence *= EVIDENCE_DECAY;
            self.bypass_evidence *= EVIDENCE_DECAY;
        }

        match self.regime {
            EchoRegime::Engaged if self.bypass_evidence >= BYPASS_AT => {
                self.regime = EchoRegime::Bypassed;
                self.bypass_evidence = 0.0;
                log::info!(
                    "[aec] no acoustic echo path detected (corr={corr:.2}, mic quiet while far-end active); bypassing echo cancellation"
                );
            }
            EchoRegime::Bypassed if self.engage_evidence >= ENGAGE_AT => {
                self.regime = EchoRegime::Engaged;
                self.engage_evidence = 0.0;
                log::info!(
                    "[aec] acoustic echo path detected (corr={corr:.2}); engaging echo cancellation"
                );
            }
            _ => {}
        }
    }
}

fn lagged_envelope_corr(near: &[f32], far: &[Option<f32>], lag: usize) -> Option<f32> {
    let mut n = 0usize;
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for t in lag..near.len() {
        let Some(f) = far[t - lag] else { continue };
        let x = near[t] as f64;
        let y = f as f64;
        n += 1;
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
    }
    if n < MIN_CORR_PAIRS {
        return None;
    }
    let nf = n as f64;
    let cov = sxy - sx * sy / nf;
    let vx = sxx - sx * sx / nf;
    let vy = syy - sy * sy / nf;
    if vx <= EPS as f64 || vy <= EPS as f64 {
        return None;
    }
    Some((cov / (vx * vy).sqrt()) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);

    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f32) / (u32::MAX >> 1) as f32
        }
    }

    fn burst_envelope(blocks: usize, seed: u64, duty: f32, amp: f32) -> Vec<f32> {
        let mut rng = Lcg(seed);
        let seg = 40;
        let mut env = vec![0.0f32; blocks];
        let mut i = 0;
        while i < blocks {
            let level = if rng.next_f32() < duty {
                amp * (0.3 + 0.7 * rng.next_f32())
            } else {
                0.0
            };
            for j in i..(i + seg).min(blocks) {
                env[j] = level;
            }
            i += seg;
        }
        env
    }

    fn feed(det: &mut EchoPathDetector, near: &[f32], far: &[f32], noise: f32) {
        for (n, f) in near.iter().zip(far.iter()) {
            det.push_block(n + noise, Some(*f));
        }
    }

    #[test]
    fn speaker_echo_engages_from_bypassed() {
        let blocks = 2000;
        let far = burst_envelope(blocks, 1, 0.6, 0.2);
        let delay = 5;
        let near: Vec<f32> = (0..blocks)
            .map(|i| {
                if i >= delay {
                    far[i - delay] * 0.1
                } else {
                    0.0
                }
            })
            .collect();

        let mut det = EchoPathDetector::new(EchoPathPrior::Clear);
        assert_eq!(det.regime(), EchoRegime::Bypassed);
        feed(&mut det, &near, &far, 1e-4);
        assert_eq!(det.regime(), EchoRegime::Engaged);
    }

    #[test]
    fn silent_mic_with_active_far_bypasses() {
        let blocks = 4000;
        let far = burst_envelope(blocks, 2, 0.6, 0.2);
        let near = vec![0.0f32; blocks];

        let mut det = EchoPathDetector::new(EchoPathPrior::Unknown);
        assert_eq!(det.regime(), EchoRegime::Engaged);
        feed(&mut det, &near, &far, 1e-4);
        assert_eq!(det.regime(), EchoRegime::Bypassed);
    }

    #[test]
    fn independent_near_speech_stays_engaged() {
        let blocks = 4000;
        let far = burst_envelope(blocks, 3, 0.6, 0.2);
        let near = burst_envelope(blocks, 99, 0.9, 0.15);

        let mut det = EchoPathDetector::new(EchoPathPrior::EchoProne);
        feed(&mut det, &near, &far, 1e-4);
        assert_eq!(det.regime(), EchoRegime::Engaged);
    }

    #[test]
    fn silent_far_produces_no_transition() {
        let blocks = 4000;
        let near = burst_envelope(blocks, 4, 0.5, 0.15);

        let mut det = EchoPathDetector::new(EchoPathPrior::Unknown);
        for n in &near {
            det.push_block(n + 1e-4, Some(0.0));
        }
        assert_eq!(det.regime(), EchoRegime::Engaged);
    }

    #[test]
    fn missing_far_coverage_produces_no_transition() {
        let blocks = 4000;
        let near = burst_envelope(blocks, 5, 0.5, 0.15);

        let mut det = EchoPathDetector::new(EchoPathPrior::Unknown);
        for n in &near {
            det.push_block(n + 1e-4, None);
        }
        assert_eq!(det.regime(), EchoRegime::Engaged);
    }

    #[test]
    fn echo_returning_after_bypass_reengages() {
        let blocks = 4000;
        let far = burst_envelope(blocks, 6, 0.6, 0.2);
        let mut det = EchoPathDetector::new(EchoPathPrior::Unknown);
        feed(&mut det, &vec![0.0; blocks], &far, 1e-4);
        assert_eq!(det.regime(), EchoRegime::Bypassed);

        let far2 = burst_envelope(2000, 7, 0.6, 0.2);
        let delay = 5;
        let near2: Vec<f32> = (0..2000)
            .map(|i| {
                if i >= delay {
                    far2[i - delay] * 0.1
                } else {
                    0.0
                }
            })
            .collect();
        feed(&mut det, &near2, &far2, 1e-4);
        assert_eq!(det.regime(), EchoRegime::Engaged);
    }
}
