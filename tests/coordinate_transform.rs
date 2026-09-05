use localdesktop::core::coordinate_transform::{
    CoordinateTransform, LogicalPoint, LogicalRect, PhysicalPoint, PhysicalRect, ViewportTransform,
};

fn assert_near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
}

fn transform(scale: f64) -> CoordinateTransform {
    CoordinateTransform::new(
        PhysicalRect {
            x: 37.0,
            y: 23.0,
            width: 2400.0,
            height: 1600.0,
        },
        LogicalRect {
            x: 11.0,
            y: 7.0,
            width: 2400.0 / scale,
            height: 1600.0 / scale,
        },
        ViewportTransform::Normal,
    )
    .unwrap()
}

#[test]
fn required_fractional_scales_map_edges_and_centre() {
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
        let tx = transform(scale);
        for physical in [
            PhysicalPoint { x: 37.0, y: 23.0 },
            PhysicalPoint {
                x: 1237.0,
                y: 823.0,
            },
            PhysicalPoint {
                x: 2437.0,
                y: 1623.0,
            },
        ] {
            let round_trip = tx.logical_to_physical(tx.physical_to_logical(physical));
            assert_near(round_trip.x, physical.x);
            assert_near(round_trip.y, physical.y);
        }

        let centre = tx.physical_to_logical(PhysicalPoint {
            x: 1237.0,
            y: 823.0,
        });
        assert_near(centre.x, 11.0 + 1200.0 / scale);
        assert_near(centre.y, 7.0 + 800.0 / scale);
    }
}

#[test]
fn clamps_points_to_rendered_viewport_edges() {
    let tx = transform(1.5);
    assert_eq!(
        tx.physical_to_logical(PhysicalPoint {
            x: -500.0,
            y: 9999.0
        }),
        LogicalPoint {
            x: 11.0,
            y: 7.0 + 1600.0 / 1.5
        }
    );
}

#[test]
fn every_rotation_and_reflection_is_invertible() {
    for viewport_transform in [
        ViewportTransform::Normal,
        ViewportTransform::Rotate90,
        ViewportTransform::Rotate180,
        ViewportTransform::Rotate270,
        ViewportTransform::Flipped,
        ViewportTransform::Flipped90,
        ViewportTransform::Flipped180,
        ViewportTransform::Flipped270,
    ] {
        let tx = CoordinateTransform::new(
            PhysicalRect {
                x: 101.0,
                y: 53.0,
                width: 2000.0,
                height: 1200.0,
            },
            LogicalRect {
                x: 5.0,
                y: 9.0,
                width: 1600.0,
                height: 960.0,
            },
            viewport_transform,
        )
        .unwrap();
        for physical in [
            PhysicalPoint { x: 101.0, y: 53.0 },
            PhysicalPoint {
                x: 1101.0,
                y: 653.0,
            },
            PhysicalPoint {
                x: 2101.0,
                y: 1253.0,
            },
            PhysicalPoint {
                x: 477.25,
                y: 991.75,
            },
        ] {
            let round_trip = tx.logical_to_physical(tx.physical_to_logical(physical));
            assert_near(round_trip.x, physical.x);
            assert_near(round_trip.y, physical.y);
        }
    }
}

#[test]
fn inverse_maps_logical_cursor_through_same_offset_and_scale() {
    let tx = transform(1.75);
    let physical = tx.logical_to_physical(LogicalPoint { x: 811.0, y: 407.0 });
    assert_near(physical.x, 1437.0);
    assert_near(physical.y, 723.0);
}

#[test]
fn authoritative_display_state_configure_size_invariant_under_observed_size() {
    use localdesktop::core::coordinate_transform::AuthoritativeDisplayState;

    let mut state = AuthoritativeDisplayState::new(3392, 2400, 420, 120000);
    // Baseline density scale is 420/160 = 2.625
    // configure_size() derives logical dimensions: (round(3392/2.625), round(2400/2.625)) = (1292, 914)
    assert_eq!(state.configure_size(), (1292, 914));
    assert_eq!(state.presentation_scale(), (1.0, 1.0));

    // KWin commits with scale 2.0 (surface size 1696x1200)
    let changed = state.update_observed_surface_size((1696.0, 1200.0));
    assert!(changed);
    // Crucial invariant: configure_size MUST remain (1292, 914), NEVER polluted by observed surface!
    assert_eq!(state.configure_size(), (1292, 914));
    assert_near(state.presentation_scale().0, 2.0);
    assert_near(state.presentation_scale().1, 2.0);

    // Update to authoritative Plasma scale 2.0
    state.update_kwin_scale(2.0);
    // Now logical configure size reflects scale 2.0: 3392/2 = 1696, 2400/2 = 1200
    assert_eq!(state.configure_size(), (1696, 1200));

    // KWin commits arbitrary other observed size
    let changed2 = state.update_observed_surface_size((1000.0, 800.0));
    assert!(changed2);
    // configure_size remains rock-solid (1696, 1200)
    assert_eq!(state.configure_size(), (1696, 1200));
}

#[test]
fn logical_configure_rounding_policy_invariants() {
    use localdesktop::core::coordinate_transform::{
        kwin_logical_to_physical_pixels, physical_to_kwin_logical_configure,
    };

    let host = (3392, 2400);
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.625] {
        let logical = physical_to_kwin_logical_configure(host, scale);
        let recon_host = kwin_logical_to_physical_pixels(logical, scale);
        // Invariant: error bounded by <= 1 pixel across all supported scales
        assert!(
            (recon_host.0 - host.0).abs() <= 1,
            "scale {scale} width reconstructed {recon_host:?} vs {host:?}"
        );
        assert!(
            (recon_host.1 - host.1).abs() <= 1,
            "scale {scale} height reconstructed {recon_host:?} vs {host:?}"
        );
    }

    // Odd dimensions test
    let odd_host = (3393, 2401);
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.625] {
        let logical = physical_to_kwin_logical_configure(odd_host, scale);
        let recon = kwin_logical_to_physical_pixels(logical, scale);
        assert!((recon.0 - odd_host.0).abs() <= 1);
        assert!((recon.1 - odd_host.1).abs() <= 1);
    }
}

#[test]
fn unattributed_commit_invalidates_frame_ownership() {
    use localdesktop::core::coordinate_transform::AuthoritativeDisplayState;

    let mut state = AuthoritativeDisplayState::new(3392, 2400, 320, 120000); // 320 dpi = 2.0x
    state.update_kwin_scale(2.0);
    // Commit valid frame for current target (logical 1696x1200)
    assert!(state.note_kwin_commit(Some((1696.0, 1200.0)), None, Some(2)));
    assert!(state.presentation_snapshot().converged);
    assert!(state.rendered_is_current());

    // Commit an unattributable frame (e.g. nonsense dimensions from corrupt/stale pipe)
    assert!(state.note_kwin_commit(Some((777.0, 555.0)), None, Some(2)));
    let snap = state.presentation_snapshot();
    assert!(!snap.converged);
    assert!(!state.rendered_is_current());
    assert_eq!(snap.rendered_generation, 0);
    assert_eq!(snap.rendered_host, (0, 0));
}

#[test]
fn authoritative_display_state_pointer_and_cursor_bijection() {
    use localdesktop::core::coordinate_transform::AuthoritativeDisplayState;

    let mut state = AuthoritativeDisplayState::new(3392, 2400, 420, 120000);
    // Plasma UI scale is authoritative (never derived from stale presentation).
    state.update_kwin_scale(2.0);
    // KWin commits buffer_scale 2 → logical 1696x1200, converging the desktop.
    assert!(state.note_kwin_commit(None, Some((1696.0, 1200.0)), Some(2)));
    let tx = state.coordinate_transform();

    // Physical touch at (1000, 800)
    let log_pt = tx.physical_to_logical(PhysicalPoint {
        x: 1000.0,
        y: 800.0,
    });
    assert_near(log_pt.x, 500.0);
    assert_near(log_pt.y, 400.0);

    // Round-trip back to physical
    let phys_pt = tx.logical_to_physical(log_pt);
    assert_near(phys_pt.x, 1000.0);
    assert_near(phys_pt.y, 800.0);

    // Cursor hotspot uses the SAME uniform scale as the desktop (no X/Y split).
    let uniform = state.uniform_presentation_scale();
    assert_near(uniform, 2.0);
    let hotspot = (10.0, 10.0);
    let elem_phys = (
        phys_pt.x - hotspot.0 * uniform,
        phys_pt.y - hotspot.1 * uniform,
    );
    assert_near(elem_phys.0, 1000.0 - 20.0);
    assert_near(elem_phys.1, 800.0 - 20.0);

    // When rendered with uniform 2.0, hotspot (10, 10) lands at exactly:
    let hotspot_landed = (
        elem_phys.0 + hotspot.0 * uniform,
        elem_phys.1 + hotspot.1 * uniform,
    );
    assert_near(hotspot_landed.0, 1000.0);
    assert_near(hotspot_landed.1, 800.0);
}

#[test]
fn fractional_225_scale_pointer_alignment_and_decoupling() {
    use localdesktop::core::coordinate_transform::AuthoritativeDisplayState;

    let mut state = AuthoritativeDisplayState::new(3392, 2400, 420, 120000);
    // KWin configured with scale 2.25
    state.update_kwin_scale(2.25);
    // Because Wayland core buffer_scale is an integer, KWin commits buffer_scale 3,
    // which results in an observed surface size of 3392/3 = 1130.66, 2400/3 = 800.
    // Size-aware commit converges the desktop (guest 1508x1067) instead of merely
    // recording a texture.
    assert!(state.note_kwin_commit(None, Some((1130.0, 800.0)), Some(3)));

    // Uniform presentation scale FITs the 1130x800 texture into 3392x2400:
    // min(3392/1130, 2400/800) = 3.0 (allows downscaling, no anisotropic fill).
    let uniform = state.uniform_presentation_scale();
    assert_near(uniform, 3.0);

    // KWin logical geometry MUST be 1508x1067, NOT 1130x800!
    let (log_w, log_h) = state.logical_geometry();
    assert_eq!((log_w, log_h), (1508.0, 1067.0));

    // Single snapshot drives input + cursor + rendering together.
    let snap = state.presentation_snapshot();
    assert_eq!(snap.guest_logical, (1508.0, 1067.0));
    assert_near(snap.uniform_scale, 3.0);
    // Integer truncation (1130 vs 1130.66) leaves a ≤2px centered pillar;
    // snapshot still reports converged (within rounding tolerance).
    assert!(snap.converged);

    let tx = state.coordinate_transform();
    assert_eq!(tx.logical_source().width, 1508.0);
    assert_eq!(tx.logical_source().height, 1067.0);

    // Top-left corner (viewport origin may be ~1px in from host edge due to
    // integer rounding; transform clamps, snapshot rejects outside).
    let top_left = tx.physical_to_logical(PhysicalPoint { x: 0.0, y: 0.0 });
    assert_near(top_left.x, 0.0);
    assert_near(top_left.y, 0.0);

    // Center of screen
    let center = tx.physical_to_logical(PhysicalPoint {
        x: 1696.0,
        y: 1200.0,
    });
    assert_near(center.x, 754.0);
    assert_near(center.y, 533.5);

    // Bottom-right corner reaches full logical geometry (1508, 1067)
    let bottom_right = tx.physical_to_logical(PhysicalPoint {
        x: 3392.0,
        y: 2400.0,
    });
    assert_near(bottom_right.x, 1508.0);
    assert_near(bottom_right.y, 1067.0);

    // Round-trip back to physical pixels is exact within integer-rounding
    // pillar tolerance (≤2.5px): viewport 3390 vs host 3392.
    let back_br = tx.logical_to_physical(bottom_right);
    assert!(
        (back_br.x - 3392.0).abs() <= 2.5 && (back_br.y - 2400.0).abs() <= 2.5,
        "round-trip {:?} should be within 2.5px of fullscreen",
        back_br
    );
    // Snapshot round-trip for an interior point is exact (<1px hotspot budget).
    let (px, py) = snap.logical_to_physical(754.0, 533.5).unwrap();
    let (lx, ly) = snap.physical_to_logical(px, py).unwrap();
    assert!((lx - 754.0).abs() < 1.0 && (ly - 533.5).abs() < 1.0);
}

#[test]
fn parse_kwin_scale_from_json_test() {
    use localdesktop::core::coordinate_transform::parse_kwin_scale_from_json;

    let sample_json = r#"[
        {
            "data": [
                {
                    "connectorName": "WL-0",
                    "mode": {
                        "height": 2400,
                        "refreshRate": 60000,
                        "width": 3392
                    },
                    "scale": 2.25
                }
            ],
            "name": "outputs"
        }
    ]"#;
    assert_eq!(parse_kwin_scale_from_json(sample_json), Some(2.25));

    let sample_2x = r#"[{"name":"outputs","data":[{"scale":2}]}]"#;
    assert_eq!(parse_kwin_scale_from_json(sample_2x), Some(2.0));

    let invalid = r#"[]"#;
    assert_eq!(parse_kwin_scale_from_json(invalid), None);
}
