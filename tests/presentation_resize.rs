//! Adversarial resize/presentation robustness tests.
//!
//! Exercises the fullscreen → popup → many aspect ratios → fullscreen sequence
//! from the task, with delayed guest commits, stale-commit races, zero-size
//! surfaces and randomized property-style storms. All geometry goes through
//! [`AuthoritativeDisplayState`] + [`PresentationSnapshot`] — the same code the
//! compositor, cursor and input paths use — so these run on the host without ADB.

use localdesktop::core::coordinate_transform::AuthoritativeDisplayState;
use localdesktop::core::presentation::fit_viewport;

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

fn guest_for(host: (i32, i32)) -> (f64, f64) {
    (
        (host.0 as f64 / PLASMA).round(),
        (host.1 as f64 / PLASMA).round(),
    )
}

fn assert_near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
}

/// Request `host` like a configure send site would: bump generation, record
/// the serial so later commits attribute to this request.
fn request(state: &mut AuthoritativeDisplayState, host: (i32, i32), serial: u32) -> u64 {
    let gen = state
        .try_update_physical_size(host.0, host.1)
        .expect("valid distinct resize must bump generation");
    state.note_configure_sent(serial);
    gen
}

/// Assert an intermediate frame that really arrived after a newer host
/// request: the rendered texture AND logical both belong to `frame_host`,
/// the viewport FITs that frame into `target`, pointer/touch map that frame,
/// and the newest host/requested target is untouched.
fn assert_intermediate_frame(
    state: &AuthoritativeDisplayState,
    frame_host: (i32, i32),
    target: (i32, i32),
    context: &str,
) {
    let snap = state.presentation_snapshot();
    let committed = simulated_buffer_commit(frame_host);
    let req_logical =
        localdesktop::core::coordinate_transform::physical_to_kwin_logical_configure(
            target,
            state.effective_kwin_scale(),
        );
    assert_eq!(snap.host, target, "{context}: host target moved");
    assert_eq!(snap.requested, req_logical, "{context}: requested target moved");
    assert_eq!(
        snap.committed,
        Some(committed),
        "{context}: rendered texture unknown"
    );
    assert_eq!(
        snap.rendered_host, frame_host,
        "{context}: rendered origin wrong"
    );
    assert_eq!(
        snap.guest_logical,
        guest_for(frame_host),
        "{context}: logical mismatches the rendered frame (render/input skew)"
    );
    assert!(
        !snap.converged,
        "{context}: stale frame must not claim convergence"
    );
    let vp = fit_viewport(target, committed).expect("valid FIT");
    assert!(
        (snap.uniform_scale - vp.scale).abs() < 1e-9,
        "{context}: uniform scale wrong"
    );
    assert_eq!(snap.viewport_origin, vp.origin);
    assert_eq!(snap.viewport_size, vp.size);
    // Pointer/touch map THAT frame: viewport center round-trips <1px and
    // lands on that frame's logical center.
    let (cx, cy) = (vp.origin.0 + vp.size.0 / 2.0, vp.origin.1 + vp.size.1 / 2.0);
    let (lx, ly) = snap
        .physical_to_logical(cx, cy)
        .expect("viewport center must map");
    let (rx, ry) = snap.logical_to_physical(lx, ly).unwrap();
    assert!(
        ((rx - cx).powi(2) + (ry - cy).powi(2)).sqrt() < 1.0,
        "{context}: center round-trip drifted"
    );
    let g = guest_for(frame_host);
    assert!(
        (lx - g.0 / 2.0).abs() < 1.0 && (ly - g.1 / 2.0).abs() < 1.0,
        "{context}: center maps to {lx},{ly} instead of frame center"
    );
}

#[test]
fn intermediate_commit_renders_own_frame_while_targeting_newest() {
    // A → request B → request C → commit B → commit C.
    let (b, c) = ((1100, 900), (2100, 1600));
    let mut s = fresh_converged_fullscreen();
    let gen_b = request(&mut s, b, 11);
    let gen_c = request(&mut s, c, 12);
    assert!(gen_c > gen_b);
    // B really arrives after C was requested (not merely delayed-away).
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(b)), Some(BUFFER_SCALE)));
    assert_intermediate_frame(&s, b, c, "B-while-targeting-C");
    assert_eq!(s.presentation_snapshot().rendered_generation, gen_b);
    assert_eq!(s.physical_size, c, "stale commit moved the host target");
    assert_eq!(
        s.requested_configure,
        localdesktop::core::coordinate_transform::physical_to_kwin_logical_configure(c, PLASMA)
    );
    // The matching commit for the newest target converges exactly.
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(c)), Some(BUFFER_SCALE)));
    let snap = s.presentation_snapshot();
    assert!(snap.converged);
    assert_eq!(snap.guest_logical, guest_for(c));
    assert_eq!(snap.rendered_generation, gen_c);
    assert_snapshot_invariants(&s, "converged-C", PLASMA);
    assert_pointer_touch_hotspot(&s, "converged-C");
}

#[test]
fn rapid_bcd_then_old_b_then_d() {
    // A → B → C → D → commit B → commit D.
    let (b, c, d) = ((1100, 900), (2100, 1600), (850, 1200));
    let mut s = fresh_converged_fullscreen();
    request(&mut s, b, 21);
    request(&mut s, c, 22);
    let gen_d = request(&mut s, d, 23);
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(b)), Some(BUFFER_SCALE)));
    assert_intermediate_frame(&s, b, d, "B-while-targeting-D");
    // Skipped intermediates (C) never need a commit; the newest one converges.
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(d)), Some(BUFFER_SCALE)));
    let snap = s.presentation_snapshot();
    assert!(snap.converged);
    assert_eq!(snap.guest_logical, guest_for(d));
    assert_eq!(snap.rendered_generation, gen_d);
}

#[test]
fn aba_stale_intermediate_then_newest() {
    // A → B → A(newest) → stale B commit → newest A commit.
    let (a, b) = ((3392, 2400), (1100, 900));
    let mut s = fresh_converged_fullscreen();
    let gen_b = request(&mut s, b, 31);
    s.note_configure_acked(31);
    let gen_a2 = request(&mut s, a, 32);
    assert!(gen_a2 > gen_b);
    // KWin's in-flight B frame lands after A was re-requested: it must render
    // B with B-era logical (not A!), still targeting newest A.
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(b)), Some(BUFFER_SCALE)));
    assert_intermediate_frame(&s, b, a, "stale-B-while-targeting-A2");
    assert_eq!(s.presentation_snapshot().rendered_generation, gen_b);
    // The newest A frame converges and attributes to the newest A request.
    s.note_configure_acked(32);
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(a)), Some(BUFFER_SCALE)));
    let snap = s.presentation_snapshot();
    assert!(snap.converged);
    assert_eq!(snap.guest_logical, guest_for(a));
    assert_eq!(snap.rendered_generation, gen_a2);
    assert_eq!(snap.acked_serial, 32);
}

#[test]
fn duplicate_size_requests_do_not_corrupt_ownership() {
    // A → B → B(coalesced) → C → commit B → commit C.
    let (b, c) = ((1100, 900), (2100, 1600));
    let mut s = fresh_converged_fullscreen();
    let gen_b = request(&mut s, b, 41);
    let len_before = s.request_history().len();
    assert_eq!(s.try_update_physical_size(b.0, b.1), None);
    assert_eq!(s.request_history().len(), len_before);
    // Re-sending the same configure (same generation) only refreshes serials.
    s.note_configure_sent(42);
    assert_eq!(s.request_history().len(), len_before);
    let gen_c = request(&mut s, c, 43);
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(b)), Some(BUFFER_SCALE)));
    assert_intermediate_frame(&s, b, c, "dup-B-while-targeting-C");
    assert_eq!(s.presentation_snapshot().rendered_generation, gen_b);
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(c)), Some(BUFFER_SCALE)));
    assert!(s.presentation_snapshot().converged);
    assert_eq!(s.presentation_snapshot().rendered_generation, gen_c);
}

#[test]
fn repeated_dimensions_across_generations() {
    // A → B → C → B(newest): a B-sized commit belongs to the newest B.
    let (b, c) = ((1100, 900), (2100, 1600));
    let mut s = fresh_converged_fullscreen();
    request(&mut s, b, 51);
    request(&mut s, c, 52);
    let gen_b2 = request(&mut s, b, 53);
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(b)), Some(BUFFER_SCALE)));
    let snap = s.presentation_snapshot();
    assert!(snap.converged, "newest B frame fills newest B target");
    assert_eq!(snap.guest_logical, guest_for(b));
    assert_eq!(snap.rendered_generation, gen_b2);
}

#[test]
fn surface_commits_carry_authoritative_logical_across_scale_change() {
    // Viewporter path: surface size IS the KWin logical. It survives a Plasma
    // scale change verbatim, and so does every other rendered frame: a scale
    // change alone never rewrites the displayed frame's pairing.
    let h = (1600, 1200);
    let mut s = AuthoritativeDisplayState::new(h.0, h.1, DENSITY_DPI, 144_000);
    s.update_kwin_scale(PLASMA);
    let surface = (711.0, 533.0); // round(1600/2.25), round(1200/2.25)
    let buffer = simulated_buffer_commit(h);
    assert!(s.note_kwin_commit(Some(surface), Some(buffer), Some(BUFFER_SCALE)));
    assert!(s.presentation_snapshot().converged);
    assert_eq!(s.logical_geometry(), surface);
    assert!(s.rendered_from_surface);
    // Plasma scale change: the surface frame keeps KWin's own logical.
    assert!(s.update_kwin_scale(2.0));
    assert_eq!(s.logical_geometry(), surface);
    // A buffer-derived frame likewise keeps its OLD logical: the displayed
    // old texture retains its committed geometry until a genuine new commit.
    let mut t = AuthoritativeDisplayState::new(h.0, h.1, DENSITY_DPI, 144_000);
    t.update_kwin_scale(PLASMA);
    assert!(t.note_kwin_commit(None, Some(buffer), Some(BUFFER_SCALE)));
    assert_eq!(t.logical_geometry(), guest_for(h));
    assert!(t.update_kwin_scale(2.0));
    assert_eq!(
        t.logical_geometry(),
        guest_for(h),
        "old frame keeps old logical across scale change"
    );
    // The genuine new commit then pairs atomically with the new scale.
    let new_guest = ((h.0 as f64 / 2.0).round(), (h.1 as f64 / 2.0).round());
    assert!(t.note_kwin_commit(None, Some(buffer), Some(BUFFER_SCALE)));
    assert_eq!(t.logical_geometry(), new_guest);
}

#[test]
fn surface_mirroring_buffer_derives_logical_live_device_case() {
    // Observed live on the OnePlus Pad 3 at Plasma 2.25x: KWin sets no
    // viewport, so `surface_size() == buffer_size() == 1130x800`. That surface
    // reading is a buffer mirror, NOT KWin logical — using it verbatim would
    // pair the fullscreen texture with a 1130x800 desktop (render/input skew).
    let mut s = AuthoritativeDisplayState::new(3392, 2400, DENSITY_DPI, 144_000);
    s.update_kwin_scale(PLASMA);
    let mirror = simulated_buffer_commit((3392, 2400));
    assert!(s.note_kwin_commit(Some(mirror), Some(mirror), Some(BUFFER_SCALE)));
    assert_eq!(s.logical_geometry(), guest_for((3392, 2400)));
    assert!(!s.rendered_from_surface);
    let snap = s.presentation_snapshot();
    assert!(snap.converged);
    assert_near(snap.uniform_scale, 3.0);
    // Repeat identical commits (every frame) report no change: no log spam,
    // no transform churn.
    assert!(!s.note_kwin_commit(Some(mirror), Some(mirror), Some(BUFFER_SCALE)));
    assert!(!s.note_kwin_commit(Some(mirror), Some(mirror), Some(BUFFER_SCALE)));
}

#[test]
fn ack_without_commit_never_reattributes_rendered_frame() {
    // A committed → request B → request A2 → ACK A2 → several evaluations
    // with NO KWin commit. Rendered ownership must stay with old A throughout;
    // only a genuine new commit may move it. (At runtime the KwinCommitGate
    // enforces the "no note call without a commit" precondition; here the
    // test simply never calls note until the genuine commit.)
    let (a, b) = ((3392, 2400), (1100, 900));
    let mut s = fresh_converged_fullscreen();
    let gen_a = s.presentation_snapshot().rendered_generation;
    let _gen_b = request(&mut s, b, 71);
    let gen_a2 = request(&mut s, a, 72);
    assert_eq!(s.note_configure_acked(72), Some(gen_a2));
    // Several redraw-equivalent evaluations: snapshot reads only.
    for _ in 0..3 {
        let snap = s.presentation_snapshot();
        assert_eq!(snap.acked_serial, 72, "ACK is recorded");
        assert_eq!(
            snap.rendered_generation, gen_a,
            "ACK alone must not reattribute the old texture"
        );
        assert_eq!(snap.rendered_host, a);
        assert_eq!(snap.guest_logical, guest_for(a));
        assert_eq!(snap.host, a, "newest target untouched");
        assert_eq!(
            snap.requested,
            localdesktop::core::coordinate_transform::physical_to_kwin_logical_configure(a, PLASMA)
        );
        assert_eq!(snap.committed, Some(simulated_buffer_commit(a)));
        assert!(
            !snap.converged,
            "stale A1 frame must not claim convergence for A2 target before A2 commits"
        );
    }
    // The genuine new A2 commit (same dimensions, real wl_surface commit at
    // runtime) moves ownership to A2.
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(a)), Some(BUFFER_SCALE)));
    let snap = s.presentation_snapshot();
    assert_eq!(snap.rendered_generation, gen_a2);
    assert_eq!(snap.guest_logical, guest_for(a));
    assert!(snap.converged);
    // Repeating the identical commit changes nothing further.
    assert!(!s.note_kwin_commit(None, Some(simulated_buffer_commit(a)), Some(BUFFER_SCALE)));
}

#[test]
fn scale_ack_without_commit_keeps_old_frame_ownership() {
    // A (old scale) → same host dimensions → Plasma scale change + config ACK
    // → no new KWin commit. Ownership (generation/host/texture) AND the old
    // frame's logical geometry must not move merely because config metadata
    // changed.
    let a = (3392, 2400);
    let mut s = fresh_converged_fullscreen();
    let old_gen = s.presentation_snapshot().rendered_generation;
    assert!(s.update_kwin_scale(2.0));
    let new_gen = s.resize_generation;
    assert_eq!(s.note_configure_acked(99), None);
    let committed = simulated_buffer_commit(a);
    for _ in 0..3 {
        let snap = s.presentation_snapshot();
        assert_eq!(snap.rendered_generation, old_gen);
        assert_eq!(snap.rendered_host, a);
        assert_eq!(snap.committed, Some(committed));
        assert_eq!(snap.host, a);
        // The cached scale IS new, but the displayed old texture keeps the
        // logical geometry of its committed frame.
        assert!((snap.plasma_scale - 2.0).abs() < 1e-9);
        assert_eq!(snap.guest_logical, guest_for(a));
    }
    // The genuine same-size commit then pairs atomically with the new scale:
    // same entry, same texture, new logical.
    let new_guest = ((a.0 as f64 / 2.0).round(), (a.1 as f64 / 2.0).round());
    assert!(s.note_kwin_commit(None, Some(committed), Some(BUFFER_SCALE)));
    let snap = s.presentation_snapshot();
    assert_eq!(snap.rendered_generation, new_gen);
    assert_eq!(snap.rendered_host, a);
    assert_eq!(snap.guest_logical, new_guest);
    assert!(snap.converged);
    // Repeating it is then a no-op.
    assert!(!s.note_kwin_commit(None, Some(committed), Some(BUFFER_SCALE)));
}

#[test]
fn configure_serials_and_acks_attribute_commits() {
    let (b, c) = ((1100, 900), (2100, 1600));
    let mut s = fresh_converged_fullscreen();
    let gen_b = request(&mut s, b, 61);
    assert_eq!(s.note_configure_acked(61), Some(gen_b));
    let gen_c = request(&mut s, c, 62);
    // History is ordered oldest-first with serials attached.
    let history = s.request_history();
    assert!(history.len() >= 3);
    assert_eq!(history[history.len() - 2].serial, 61);
    assert_eq!(history[history.len() - 1].serial, 62);
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(b)), Some(BUFFER_SCALE)));
    assert_eq!(s.presentation_snapshot().rendered_generation, gen_b);
    assert_eq!(s.presentation_snapshot().acked_serial, 61);
    // Unknown serials never corrupt attribution.
    assert_eq!(s.note_configure_acked(9999), None);
    assert_eq!(s.presentation_snapshot().acked_serial, 9999);
    assert!(s.note_kwin_commit(None, Some(simulated_buffer_commit(c)), Some(BUFFER_SCALE)));
    assert!(s.presentation_snapshot().converged);
    assert_eq!(s.presentation_snapshot().rendered_generation, gen_c);
}

#[test]
fn request_history_is_bounded_and_newest_wins() {
    let mut s = fresh_converged_fullscreen();
    let mut serial = 100u32;
    // 20 distinct requests; ring keeps the newest 16.
    for i in 0..20 {
        let w = 1000 + i * 37;
        let h = 800 + i * 23;
        serial += 1;
        request(&mut s, (w, h), serial);
    }
    let history = s.request_history();
    assert_eq!(history.len(), 16);
    assert_eq!(history[15].host, (1000 + 19 * 37, 800 + 19 * 23));
    assert_eq!(history[15].serial, serial);
    // Genset strictly increases along the ring.
    for pair in history.windows(2) {
        assert!(pair[0].generation < pair[1].generation);
    }
}

#[test]
fn cold_startup_scale_race_repair_and_convergence() {
    // Cold startup on OnePlus Pad 3 with Plasma scale 200%.
    // KWin's nested WaylandOutput initializes at scale 1.0, applies the initial configure
    // at 1.0 (mode 1696x1200), then loads kwinoutputconfig.json (scale 2.0).
    // KWin commits 848x600 (1696 / 2).
    let mut state = AuthoritativeDisplayState::new(3392, 2400, DENSITY_DPI, 144_000);
    state.update_kwin_scale(2.0);
    assert_eq!(state.configure_size(), (1696, 1200));

    // Host sends initial configure serial 1.
    state.note_configure_sent(1);
    assert!(state.has_unacknowledged_configure());

    // KWin acks serial 1.
    state.note_configure_acked(1);
    assert!(!state.has_unacknowledged_configure());

    // KWin commits stale frame (848x600).
    let stale_commit = (848.0, 600.0);
    state.note_kwin_commit(Some(stale_commit), Some(stale_commit), Some(2));
    assert!(!state.presentation_snapshot().converged);
    assert!(state.needs_configure_repair());

    // Host sends repair configure serial 2.
    state.note_configure_sent(2);
    state.record_configure_repair();
    assert!(state.has_unacknowledged_configure());
    assert!(!state.needs_configure_repair(), "repair must not re-trigger while in flight");

    // KWin acks serial 2 under scale 2.0, resizes output mode to 3392x2400, commits 1696x1200.
    state.note_configure_acked(2);
    assert!(!state.has_unacknowledged_configure());

    let full_commit = (1696.0, 1200.0);
    state.note_kwin_commit(Some(full_commit), Some(full_commit), Some(2));
    let snap = state.presentation_snapshot();
    assert!(snap.converged);
    assert_eq!(snap.guest_logical, (1696.0, 1200.0));
    assert_eq!(snap.viewport_origin, (0.0, 0.0));
    assert_eq!(snap.viewport_size, (3392.0, 2400.0));
    assert_eq!(snap.uniform_scale, 2.0);
    assert!(!state.needs_configure_repair());
}

#[test]
fn configure_repair_stops_after_three_attempts() {
    let mut state = AuthoritativeDisplayState::new(3392, 2400, DENSITY_DPI, 144_000);
    state.update_kwin_scale(2.0);
    state.note_configure_sent(1);
    state.note_configure_acked(1);

    let rogue_commit = (800.0, 600.0);
    for serial in 2..=4 {
        state.note_kwin_commit(Some(rogue_commit), Some(rogue_commit), Some(2));
        assert!(state.needs_configure_repair());
        state.note_configure_sent(serial);
        state.record_configure_repair();
        state.note_configure_acked(serial);
    }

    // 4th commit with mismatch: attempts capped at 3, must not trigger further repairs.
    state.note_kwin_commit(Some(rogue_commit), Some(rogue_commit), Some(2));
    assert!(!state.needs_configure_repair(), "repair must be capped at 3 attempts");
}

#[test]
fn resize_in_flight_does_not_trigger_premature_repair() {
    let mut state = fresh_converged_fullscreen();
    // User resizes to popup (1134x2016).
    let popup = (1134, 2016);
    let gen_popup = state.try_update_physical_size(popup.0, popup.1).expect("new gen");
    state.note_configure_sent(101);
    assert!(state.has_unacknowledged_configure());

    // Old fullscreen frame commits while popup configure is in flight.
    let old_frame = (1696.0, 1200.0);
    state.note_kwin_commit(Some(old_frame), Some(old_frame), Some(2));
    assert!(!state.presentation_snapshot().converged);
    assert!(!state.needs_configure_repair(), "in-flight configure must suppress repair");

    // Popup configure acked and committed.
    state.note_configure_acked(101);
    let popup_commit = (567.0, 1008.0);
    state.note_kwin_commit(Some(popup_commit), Some(popup_commit), Some(2));
    let snap = state.presentation_snapshot();
    assert!(snap.converged);
    assert_eq!(snap.rendered_generation, gen_popup);
    assert!(!state.needs_configure_repair());
}

