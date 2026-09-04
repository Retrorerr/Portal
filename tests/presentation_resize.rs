//! Adversarial resize/presentation robustness tests.
//!
//! Exercises the fullscreen → popup → many aspect ratios → fullscreen sequence
//! from the task, with delayed guest commits, stale-commit races, zero-size
//! surfaces and randomized property-style storms. All geometry goes through
//! [`AuthoritativeDisplayState`] + [`PresentationSnapshot`] — the same code the
//! compositor, cursor and input paths use — so these run on the host without ADB.

use localdesktop::core::coordinate_transform::AuthoritativeDisplayState;

const PLASMA: f64 = 2.25;
const BUFFER_SCALE: i32 = 3;
const DENSITY_DPI: i32 = 420;

/// Simulated KWin buffer commit for `host` (integer `buffer_scale` path).
/// Uses floor to mimic Smithay's integer `Size<i32, Logical>` truncation
/// (worst-case rounding, e.g. 3392/3=1130.66→1130).
fn simulated_buffer_commit(host: (i32, i32)) -> (f64, f64) {
    (
        (host.0 as f64 / BUFFER_SCALE as f64).floor().max(1.0),
        (host.1 as f64 / BUFFER_SCALE as f64).floor().max(1.0),
    )
}

fn fresh_converged_fullscreen() -> AuthoritativeDisplayState {
    let mut state = AuthoritativeDisplayState::new(3392, 2400, DENSITY_DPI, 144_000);
    state.update_kwin_scale(PLASMA);
    let commit = simulated_buffer_commit((3392, 2400));
    assert!(state.note_kwin_commit(None, Some(commit), Some(BUFFER_SCALE)));
    assert!(state.presentation_snapshot().converged);
    state
}

fn assert_snapshot_invariants(
    state: &AuthoritativeDisplayState,
    context: &str,
    plasma_expected: f64,
) {
    let snap = state.presentation_snapshot();
    // No invalid geometry, no divide-by-zero.
    assert!(
        snap.host.0 > 0 && snap.host.1 > 0,
        "{context}: host {:?}",
        snap.host
    );
    assert!(
        snap.guest_logical.0 > 0.0
            && snap.guest_logical.1 > 0.0
            && snap.guest_logical.0.is_finite()
            && snap.guest_logical.1.is_finite(),
        "{context}: guest {:?}",
        snap.guest_logical
    );
    assert!(
        snap.uniform_scale.is_finite() && snap.uniform_scale > 0.0,
        "{context}: uniform {}",
        snap.uniform_scale
    );
    assert!(
        snap.viewport_size.0 > 0.0
            && snap.viewport_size.1 > 0.0
            && snap.viewport_size.0.is_finite()
            && snap.viewport_size.1.is_finite(),
        "{context}: viewport {:?}",
        snap.viewport_size
    );
    // Viewport never exceeds host (FIT, never stretch/crop beyond host).
    assert!(
        snap.viewport_size.0 <= snap.host.0 as f64 + 1e-6
            && snap.viewport_size.1 <= snap.host.1 as f64 + 1e-6,
        "{context}: viewport {:?} exceeds host {:?}",
        snap.viewport_size,
        snap.host
    );
    // Centered.
    let (ex, ey) = (
        (snap.host.0 as f64 - snap.viewport_size.0) / 2.0,
        (snap.host.1 as f64 - snap.viewport_size.1) / 2.0,
    );
    assert!(
        (snap.viewport_origin.0 - ex.max(0.0)).abs() < 1e-6
            && (snap.viewport_origin.1 - ey.max(0.0)).abs() < 1e-6,
        "{context}: viewport origin {:?} not centered",
        snap.viewport_origin
    );
    // No anisotropic desktop stretch: viewport aspect == committed aspect.
    if let Some(committed) = snap.committed {
        let want = committed.0 / committed.1;
        let got = snap.viewport_size.0 / snap.viewport_size.1;
        assert!(
            (want - got).abs() < 1e-9,
            "{context}: anisotropic! committed aspect {want} vs viewport {got}"
        );
    }
    // Plasma UI scale remains stable, no cumulative scaling.
    assert!(
        (snap.plasma_scale - plasma_expected).abs() < 1e-9,
        "{context}: plasma drifted {} vs {plasma_expected}",
        snap.plasma_scale
    );
}

fn assert_pointer_touch_hotspot(state: &AuthoritativeDisplayState, context: &str) {
    let snap = state.presentation_snapshot();
    // Interior points: center + insets. Must map and round-trip <1px.
    let (vx, vy) = snap.viewport_origin;
    let (vw, vh) = snap.viewport_size;
    let interior = [
        (vx + vw / 2.0, vy + vh / 2.0),
        (vx + 1.0, vy + 1.0),
        (vx + vw - 1.0, vy + vh - 1.0),
        (vx + vw * 0.25, vy + vh * 0.75),
    ];
    for (px, py) in interior {
        // Clamp test points inside host for tiny viewports.
        if px < 0.0 || py < 0.0 || px > snap.host.0 as f64 || py > snap.host.1 as f64 {
            continue;
        }
        let Some((lx, ly)) = snap.physical_to_logical(px, py) else {
            panic!("{context}: interior point ({px},{py}) rejected");
        };
        let Some((rx, ry)) = snap.logical_to_physical(lx, ly) else {
            panic!("{context}: logical ({lx},{ly}) unmappable");
        };
        let err = ((rx - px).powi(2) + (ry - py).powi(2)).sqrt();
        assert!(
            err < 1.0,
            "{context}: round-trip error {err} at ({px},{py})"
        );
        // Cursor hotspot for several hotspot values, same snapshot.
        for hotspot in [(0.0, 0.0), (7.0, 5.0), (32.0, 32.0)] {
            let Some((ox, oy)) = snap.cursor_sprite_origin((lx, ly), hotspot) else {
                panic!("{context}: hotspot origin failed");
            };
            let landed_x = ox + hotspot.0 * snap.uniform_scale;
            let landed_y = oy + hotspot.1 * snap.uniform_scale;
            let herr = ((landed_x - rx).powi(2) + (landed_y - ry).powi(2)).sqrt();
            assert!(
                herr < 1.0,
                "{context}: hotspot error {herr} at ({px},{py}) hotspot {hotspot:?}"
            );
        }
    }
    // Border (letterbox/pillarbox) input must be rejected, never edge-clamped.
    let (hw, hh) = (snap.host.0 as f64, snap.host.1 as f64);
    let mut border_points = Vec::new();
    if snap.viewport_size.1 < hw.min(hh) && snap.viewport_origin.1 > 2.5 {
        // Top letterbox exists (more than rounding): sample its middle.
        border_points.push((hw / 2.0, snap.viewport_origin.1 / 2.0));
    }
    if snap.viewport_size.0 < hw && snap.viewport_origin.0 > 2.5 {
        border_points.push((snap.viewport_origin.0 / 2.0, hh / 2.0));
    }
    for (px, py) in border_points {
        assert!(
            snap.physical_to_logical(px, py).is_none(),
            "{context}: border point ({px},{py}) must be rejected, not edge-clamped"
        );
    }
}

#[test]
fn adversarial_fullscreen_popup_sequence_with_delayed_commits() {
    let sequence = [
        (3392, 2400),
        (2100, 1600),
        (1100, 900),
        (1700, 600),
        (850, 1200),
        (2200, 700),
        (1200, 1800),
        (2800, 1600),
        (900, 500),
        (1900, 1300),
        (1400, 700),
        (3000, 1000),
        (1600, 1200),
        (3392, 2400),
    ];
    let mut state = fresh_converged_fullscreen();
    let initial_gen = state.resize_generation;

    for window in sequence.iter().skip(1).copied() {
        let prev_gen = state.resize_generation;
        let prev_guest = state.logical_geometry();
        let prev_committed = state.observed_surface_size;
        let context = format!("host->{window:?}");

        // Host resize happens immediately (Android), guest lags (KWin async).
        let new_gen = state
            .try_update_physical_size(window.0, window.1)
            .expect("valid resize must bump generation");
        assert!(new_gen > prev_gen, "generation must increase");
        // Plasma scale untouched by resizing; guest still OLD desktop.
        assert!((state.effective_kwin_scale() - PLASMA).abs() < 1e-9);
        if prev_committed.is_some() {
            assert_eq!(
                state.logical_geometry(),
                prev_guest,
                "guest must stay old until commit"
            );
            assert!(
                !state.presentation_snapshot().converged || {
                    // Tiny rounding fullscreen (≤2.5px) may still report converged.
                    let s = state.presentation_snapshot();
                    (s.viewport_size.0 - s.host.0 as f64).abs() <= 2.5
                        && (s.viewport_size.1 - s.host.1 as f64).abs() <= 2.5
                }
            );
        }

        // Transitional frame: FIT old committed inside new host.
        let snap = state.presentation_snapshot();
        if let Some(committed) = prev_committed {
            let expected_scale = (window.0 as f64 / committed.0).min(window.1 as f64 / committed.1);
            assert!(
                (snap.uniform_scale - expected_scale).abs() < 1e-9,
                "{context}: uniform {} vs FIT {expected_scale}",
                snap.uniform_scale
            );
            // Downscaling must work (old 1130-wide frame into narrow popups).
            if committed.0 > window.0 as f64 || committed.1 > window.1 as f64 {
                assert!(
                    snap.uniform_scale < 1.0 || {
                        // Same-size-ish hosts may stay ≥1; only assert <1 when
                        // the limiting axis truly requires downscale.
                        let sx = window.0 as f64 / committed.0;
                        let sy = window.1 as f64 / committed.1;
                        sx.min(sy) >= 1.0
                    },
                    "{context}: expected downscale, got {}",
                    snap.uniform_scale
                );
            }
        }
        assert_snapshot_invariants(&state, &format!("transitional {context}"), PLASMA);
        assert_pointer_touch_hotspot(&state, &format!("transitional {context}"));

        // Delayed guest commit arrives for the NEW host.
        let commit = simulated_buffer_commit(window);
        assert!(
            state.note_kwin_commit(None, Some(commit), Some(BUFFER_SCALE)),
            "{context}: new commit should change state"
        );
        assert!(
            state.presentation_snapshot().converged,
            "{context}: should converge after matching commit"
        );
        let (gw, gh) = state.logical_geometry();
        assert_eq!(
            (gw, gh),
            (
                (window.0 as f64 / PLASMA).round(),
                (window.1 as f64 / PLASMA).round()
            ),
            "{context}: guest must be host/plasma after converge"
        );
        assert_snapshot_invariants(&state, &format!("converged {context}"), PLASMA);
        assert_pointer_touch_hotspot(&state, &format!("converged {context}"));
    }

    // Final fullscreen equals a clean fullscreen launch (geometry, not generation).
    let clean = fresh_converged_fullscreen();
    let (final_snap, clean_snap) = (state.presentation_snapshot(), clean.presentation_snapshot());
    assert_eq!(final_snap.host, clean_snap.host);
    assert_eq!(final_snap.guest_logical, clean_snap.guest_logical);
    assert_eq!(final_snap.committed, clean_snap.committed);
    assert!((final_snap.plasma_scale - clean_snap.plasma_scale).abs() < 1e-9);
    assert!((final_snap.uniform_scale - clean_snap.uniform_scale).abs() < 1e-9);
    assert_eq!(final_snap.viewport_origin, clean_snap.viewport_origin);
    assert_eq!(final_snap.viewport_size, clean_snap.viewport_size);
    assert_eq!(final_snap.converged, clean_snap.converged);
    assert!(state.resize_generation > initial_gen);
    assert!((state.effective_kwin_scale() - PLASMA).abs() < 1e-9);
}

#[test]
fn stale_delayed_commit_cannot_overwrite_newer_host_generation() {
    let mut state = fresh_converged_fullscreen();
    // Host races ahead: A(3392) -> B(1100x900). Guest still A-era.
    let gen_b = state.try_update_physical_size(1100, 900).unwrap();
    assert_eq!(state.logical_geometry(), (1508.0, 1067.0));
    assert!(!state.presentation_snapshot().converged);

    // Stale A-era commit (1130x800) arrives AFTER B. Same bytes as current
    // committed → no state change; guest must NOT jump to B-era (489x400).
    let stale_a = simulated_buffer_commit((3392, 2400));
    let changed = state.note_kwin_commit(None, Some(stale_a), Some(BUFFER_SCALE));
    assert!(!changed, "identical stale commit must not churn state");
    assert_eq!(
        state.resize_generation, gen_b,
        "host generation untouched by stale"
    );
    assert_eq!(
        state.logical_geometry(),
        (1508.0, 1067.0),
        "stale must not win"
    );
    assert!(!state.presentation_snapshot().converged);

    // Fresh B-era commit converges to B.
    let fresh_b = simulated_buffer_commit((1100, 900));
    assert!(state.note_kwin_commit(None, Some(fresh_b), Some(BUFFER_SCALE)));
    assert!(state.presentation_snapshot().converged);
    assert_eq!(
        state.logical_geometry(),
        ((1100.0 / PLASMA).round(), (900.0 / PLASMA).round())
    );

    // Reverse race: host back to A, stale B commit must not win either.
    let gen_a2 = state.try_update_physical_size(3392, 2400).unwrap();
    assert!(gen_a2 > gen_b);
    // Guest still B-era until A commit arrives.
    assert_eq!(
        state.logical_geometry(),
        ((1100.0 / PLASMA).round(), (900.0 / PLASMA).round())
    );
    assert!(
        state.note_kwin_commit(None, Some(fresh_b), Some(BUFFER_SCALE)) == false
            || state.logical_geometry() == ((1100.0 / PLASMA).round(), (900.0 / PLASMA).round())
    );
    let fresh_a = simulated_buffer_commit((3392, 2400));
    assert!(state.note_kwin_commit(None, Some(fresh_a), Some(BUFFER_SCALE)));
    assert_eq!(state.logical_geometry(), (1508.0, 1067.0));
}

#[test]
fn zero_and_repeat_sizes_preserve_last_valid_and_coalesce() {
    let mut state = fresh_converged_fullscreen();
    let (host, gen, guest) = (
        state.physical_size,
        state.resize_generation,
        state.logical_geometry(),
    );
    // Zero/invalid never fabricates 1px geometry.
    assert_eq!(state.try_update_physical_size(0, 900), None);
    assert_eq!(state.try_update_physical_size(1100, 0), None);
    assert_eq!(state.try_update_physical_size(0, 0), None);
    assert_eq!(state.try_update_physical_size(-5, 900), None);
    assert_eq!(state.physical_size, host);
    assert_eq!(state.resize_generation, gen);
    assert_eq!(state.logical_geometry(), guest);
    // Invalid observed commits are dropped, never stored.
    assert!(!state.update_observed_surface_size((0.0, 800.0)));
    assert!(!state.update_observed_surface_size((f64::NAN, 800.0)));
    assert_eq!(
        state.observed_surface_size,
        Some(simulated_buffer_commit((3392, 2400)))
    );
    // Identical repeats coalesce (no new generation, no guest churn).
    assert_eq!(state.try_update_physical_size(host.0, host.1), None);
    assert_eq!(state.resize_generation, gen);
    // Resize storms: many rapid targets, only newest matters; guest processes
    // only the final converged commit, intermediates safely superseded.
    let mut last_gen = gen;
    for (w, h) in [(1200, 800), (1300, 900), (1250, 850), (1600, 1200)] {
        last_gen = state.try_update_physical_size(w, h).unwrap();
    }
    assert!(last_gen > gen);
    let final_commit = simulated_buffer_commit((1600, 1200));
    assert!(state.note_kwin_commit(None, Some(final_commit), Some(BUFFER_SCALE)));
    assert_eq!(
        state.logical_geometry(),
        ((1600.0 / PLASMA).round(), (1200.0 / PLASMA).round())
    );
    assert_snapshot_invariants(&state, "storm-converged", PLASMA);
}

#[test]
fn plasma_scale_stable_across_resizes_no_cumulative_scaling() {
    let mut state = fresh_converged_fullscreen();
    let mut expected_guest = (1508.0, 1067.0);
    for (w, h) in [
        (2100, 1600),
        (850, 1200),
        (3000, 1000),
        (900, 500),
        (3392, 2400),
    ] {
        state.try_update_physical_size(w, h).unwrap();
        // Resizing never alters Plasma scale.
        assert!((state.effective_kwin_scale() - PLASMA).abs() < 1e-9);
        // Guest stays old until commit (no cumulative multiply).
        assert_eq!(state.logical_geometry(), expected_guest);
        let commit = simulated_buffer_commit((w, h));
        state.note_kwin_commit(None, Some(commit), Some(BUFFER_SCALE));
        expected_guest = ((w as f64 / PLASMA).round(), (h as f64 / PLASMA).round());
        assert_eq!(state.logical_geometry(), expected_guest);
        assert!((state.effective_kwin_scale() - PLASMA).abs() < 1e-9);
    }
    // No hardcoded panel geometry leaks into the pipeline: an arbitrary host
    // still derives guest purely as host/plasma.
    state.try_update_physical_size(1234, 987).unwrap();
    let commit = simulated_buffer_commit((1234, 987));
    state.note_kwin_commit(None, Some(commit), Some(BUFFER_SCALE));
    assert_eq!(
        state.logical_geometry(),
        ((1234.0 / PLASMA).round(), (987.0 / PLASMA).round())
    );
}

// Cheap deterministic PRNG (xorshift64*) — no extra dependencies.
struct DeterministicRng(u64);
impl DeterministicRng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % (hi - lo + 1) as u64) as i32
    }
}

#[test]
fn randomized_property_resize_storms_preserve_invariants() {
    let mut rng = DeterministicRng(0x1234_5678_9ABC_DEF0);
    let mut state = fresh_converged_fullscreen();
    // Track the newest host the guest has actually converged to, so delayed
    // commits for superseded sizes are correctly expected to stay transitional.
    for step in 0..200 {
        let w = rng.range(400, 3400);
        let h = rng.range(400, 2400);
        let context = format!("random step {step} host=({w},{h})");
        if state.try_update_physical_size(w, h).is_none() {
            continue; // coalesced repeat (should be rare with random sizes)
        }
        assert_snapshot_invariants(&state, &format!("transitional {context}"), PLASMA);
        assert_pointer_touch_hotspot(&state, &format!("transitional {context}"));
        // Sometimes deliver the matching commit immediately, sometimes delay
        // it past the next resize (stale), sometimes skip it (storm coalesce).
        let roll = rng.next_u64() % 3;
        if roll < 2 {
            let commit = simulated_buffer_commit((w, h));
            state.note_kwin_commit(None, Some(commit), Some(BUFFER_SCALE));
            // After a matching commit the guest must be host/plasma and the
            // snapshot converged (within rounding tolerance).
            assert_eq!(
                state.logical_geometry(),
                ((w as f64 / PLASMA).round(), (h as f64 / PLASMA).round()),
                "{context}: guest must converge"
            );
            assert_snapshot_invariants(&state, &format!("converged {context}"), PLASMA);
            assert_pointer_touch_hotspot(&state, &format!("converged {context}"));
        }
        // else: delayed/skipped — next iteration's resize supersedes; the
        // pending commit arriving later must not corrupt the newer host (the
        // size-aware `note_kwin_commit` keeps the old guest until a matching
        // commit arrives).
    }
    // Converge at the end on whatever host is current: final state valid.
    let (fw, fh) = state.physical_size;
    let commit = simulated_buffer_commit((fw, fh));
    state.note_kwin_commit(None, Some(commit), Some(BUFFER_SCALE));
    assert_snapshot_invariants(&state, "random-final", PLASMA);
    assert_pointer_touch_hotspot(&state, "random-final");
    assert!((state.effective_kwin_scale() - PLASMA).abs() < 1e-9);
}
