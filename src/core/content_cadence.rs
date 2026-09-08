//! Sustained compositor activity is evidence of demand, not a video's source FPS.
//! In particular, a 50 Hz Android queue may throttle a 60 fps source to 50 commits.
//! Never label this estimate FIXED_SOURCE or rewrite the nominal Wayland mode.
#[derive(Debug, Default)]
pub struct ContentCadence {
    start_ms: Option<u64>,
    frames: u64,
    last_busy_ms: u64,
    last_change_ms: u64,
    requested: Option<i32>,
}

impl ContentCadence {
    pub fn committed(&mut self, now_ms: u64) {
        self.start_ms.get_or_insert(now_ms);
        self.frames += 1;
    }

    pub fn evaluate(&mut self, now_ms: u64, nominal: i32, supported: &[i32]) -> Option<i32> {
        let start = *self.start_ms.get_or_insert(now_ms);
        let elapsed = now_ms.saturating_sub(start);
        if elapsed < 2000 {
            return None;
        }
        let fps = self.frames as f64 * 1000.0 / elapsed as f64;
        self.frames = 0;
        self.start_ms = Some(now_ms);
        let busy = (45.0..=65.0).contains(&fps);
        if busy {
            self.last_busy_ms = now_ms;
        }
        // Prefer the highest supported 60 Hz multiple: keeps UI headroom while
        // avoiding 50/90/144 Hz pull-down for sustained approximately 60 fps demand.
        let compatible = supported
            .iter()
            .copied()
            .filter(|r| {
                *r <= nominal && *r >= 59_500 && (*r % 60_000).min(60_000 - *r % 60_000) <= 500
            })
            .max();
        let current = self.requested.unwrap_or(nominal);
        let target = if busy {
            compatible.unwrap_or(nominal)
        } else if now_ms.saturating_sub(self.last_busy_ms) < 6000 {
            current
        } else {
            nominal
        };
        if target == current || now_ms.saturating_sub(self.last_change_ms) < 4000 {
            return None;
        }
        self.requested = Some(target);
        self.last_change_ms = now_ms;
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MODES: &[i32] = &[50_000, 60_000, 90_000, 120_000, 144_000];
    #[test]
    fn throttled_video_requests_compatible_rate_without_mode_changes() {
        let mut policy = ContentCadence::default();
        for t in (0..4000).step_by(20) {
            policy.committed(t);
        }
        assert_eq!(policy.evaluate(4000, 144_000, MODES), Some(120_000));
        assert_eq!(policy.evaluate(6000, 144_000, MODES), None);
        assert_eq!(policy.evaluate(10000, 144_000, MODES), Some(144_000));
    }
    #[test]
    fn short_bursts_and_unsupported_modes_do_not_change_request() {
        let mut policy = ContentCadence::default();
        policy.committed(0);
        assert_eq!(policy.evaluate(4000, 144_000, MODES), None);
        for t in (4000..8000).step_by(16) {
            policy.committed(t);
        }
        assert_eq!(policy.evaluate(8000, 90_000, &[50_000, 90_000]), None);
    }
}
