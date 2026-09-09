//! Low-overhead aggregate diagnostics for the nested desktop presentation path.

#[derive(Debug)]
pub struct PipelineStats {
    started: std::time::Instant,
    kwin_commits: u64,
    renders: u64,
    submissions: u64,
    no_damage: u64,
    damage_pixels: u128,
    upload_calls: u64,
    upload_full: u64,
    upload_partial: u64,
    upload_bytes: u128,
    upload_ns: u128,
    render_ns: u128,
    swap_ns: u128,
    presents: u64,
    present_queue_ns: u128,
}

impl Default for PipelineStats {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            kwin_commits: 0,
            renders: 0,
            submissions: 0,
            no_damage: 0,
            damage_pixels: 0,
            upload_calls: 0,
            upload_full: 0,
            upload_partial: 0,
            upload_bytes: 0,
            upload_ns: 0,
            render_ns: 0,
            swap_ns: 0,
            presents: 0,
            present_queue_ns: 0,
        }
    }
}

impl PipelineStats {
    pub fn note_kwin_commit(&mut self) {
        self.kwin_commits += 1;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn note_render(
        &mut self,
        render_ns: u64,
        damage_pixels: u64,
        upload_calls: u64,
        upload_full: u64,
        upload_partial: u64,
        upload_bytes: u64,
        upload_ns: u64,
    ) {
        self.renders += 1;
        self.render_ns += render_ns as u128;
        self.damage_pixels += damage_pixels as u128;
        self.upload_calls += upload_calls;
        self.upload_full += upload_full;
        self.upload_partial += upload_partial;
        self.upload_bytes += upload_bytes as u128;
        self.upload_ns += upload_ns as u128;
    }

    pub fn note_submit(&mut self, swap_ns: u64) {
        self.submissions += 1;
        self.swap_ns += swap_ns as u128;
    }

    pub fn note_no_damage(&mut self) {
        self.no_damage += 1;
    }

    pub fn note_present(&mut self, queue_ns: u64) {
        self.presents += 1;
        self.present_queue_ns += queue_ns as u128;
    }

    pub fn maybe_report(&mut self, requested_millihz: i32, observed_millihz: i32) {
        let elapsed = self.started.elapsed();
        if elapsed < std::time::Duration::from_secs(5) {
            return;
        }
        let seconds = elapsed.as_secs_f64().max(0.001);
        let avg = |total: u128, count: u64| -> u64 {
            if count == 0 {
                0
            } else {
                (total / count as u128 / 1_000) as u64
            }
        };
        let composition_ns = self.render_ns.saturating_sub(self.upload_ns);
        log::info!(
            "pipeline requested_hz={:.3} observed_hz={:.3} kwin_commit_hz={:.2} render_hz={:.2} submit_hz={:.2} no_damage={} damage_mp={:.2} upload_calls={} full={} partial={} upload_mb={:.2} upload_avg_us={} gles_other_avg_us={} swap_avg_us={} presents={} queue_avg_us={}",
            requested_millihz as f64 / 1000.0,
            observed_millihz as f64 / 1000.0,
            self.kwin_commits as f64 / seconds,
            self.renders as f64 / seconds,
            self.submissions as f64 / seconds,
            self.no_damage,
            self.damage_pixels as f64 / 1_000_000.0,
            self.upload_calls,
            self.upload_full,
            self.upload_partial,
            self.upload_bytes as f64 / (1024.0 * 1024.0),
            avg(self.upload_ns, self.upload_calls),
            avg(composition_ns, self.renders),
            avg(self.swap_ns, self.submissions),
            self.presents,
            avg(self.present_queue_ns, self.presents),
        );
        *self = Self::default();
    }
}
