#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AecStats {
    pub observed_blocks: u64,
    pub far_active_blocks: u64,
    pub inference_blocks: u64,
    pub unaligned_samples: u64,
    pub gate_windows: u64,
    pub gate_gated: u64,
    pub gate_mean_atten_db: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GateHistory {
    windows: u64,
    gated: u64,
    atten_db_sum: f32,
}

impl GateHistory {
    pub(crate) fn fold(&mut self, summary: (u64, u64, f32)) {
        let (windows, gated, mean_atten_db) = summary;
        self.windows += windows;
        self.gated += gated;
        self.atten_db_sum += mean_atten_db * gated as f32;
    }

    pub(crate) fn combined(&self, current: (u64, u64, f32)) -> (u64, u64, f32) {
        let mut total = *self;
        total.fold(current);
        let mean = if total.gated > 0 {
            total.atten_db_sum / total.gated as f32
        } else {
            0.0
        };
        (total.windows, total.gated, mean)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_summaries_from_before_a_route_change_are_kept() {
        let mut history = GateHistory::default();
        history.fold((10, 2, -6.0));
        assert_eq!(history.combined((5, 2, -12.0)), (15, 4, -9.0));
    }

    #[test]
    fn a_gate_that_never_fired_reports_no_attenuation() {
        assert_eq!(GateHistory::default().combined((8, 0, 0.0)), (8, 0, 0.0));
    }
}
