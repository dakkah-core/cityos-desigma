//! Easing curves for prototype transitions.

use desigma_doc::Easing;

/// CSS-style control points of the cubic-bezier based presets.
pub fn control_points(e: &Easing) -> Option<[f32; 4]> {
    Some(match e {
        Easing::Linear | Easing::Spring { .. } => return None,
        Easing::EaseIn => [0.42, 0.0, 1.0, 1.0],
        Easing::EaseOut => [0.0, 0.0, 0.58, 1.0],
        Easing::EaseInOut => [0.42, 0.0, 0.58, 1.0],
        Easing::EaseInBack => [0.36, 0.0, 0.66, -0.56],
        Easing::EaseOutBack => [0.34, 1.56, 0.64, 1.0],
        Easing::EaseInOutBack => [0.68, -0.6, 0.32, 1.6],
        Easing::Bezier(c) => *c,
    })
}

// The curve and spring math is shared with the Motion keyframe easing in desigma-doc.
pub use desigma_doc::motion::{cubic_bezier, spring_at, spring_settle_secs};

/// Eased progress for linear progress `t` in 0..1 (may overshoot for back and spring curves).
pub fn ease(e: &Easing, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match e {
        Easing::Linear => t,
        Easing::Spring { mass, stiffness, damping } => {
            if t >= 1.0 {
                1.0
            } else {
                spring_at(*mass, *stiffness, *damping, t * spring_settle_secs(*mass, *stiffness, *damping))
            }
        }
        other => control_points(other).map(|c| cubic_bezier(c, t)).unwrap_or(t),
    }
}

/// Duration actually used: springs run for their settle time, others as requested.
pub fn effective_duration_ms(e: &Easing, requested: u32) -> u32 {
    match e {
        Easing::Spring { mass, stiffness, damping } => (spring_settle_secs(*mass, *stiffness, *damping) * 1000.0) as u32,
        _ => requested,
    }
}

/// Preset names shown in the easing picker, with the curve each stands for.
pub const PRESETS: &[(&str, Easing)] = &[
    ("Linear", Easing::Linear),
    ("Ease in", Easing::EaseIn),
    ("Ease out", Easing::EaseOut),
    ("Ease in and out", Easing::EaseInOut),
    ("Ease in back", Easing::EaseInBack),
    ("Ease out back", Easing::EaseOutBack),
    ("Ease in and out back", Easing::EaseInOutBack),
    ("Gentle", Easing::Spring { mass: 1.0, stiffness: 100.0, damping: 15.0 }),
    ("Quick", Easing::Spring { mass: 1.0, stiffness: 300.0, damping: 20.0 }),
    ("Bouncy", Easing::Spring { mass: 1.0, stiffness: 600.0, damping: 15.0 }),
    ("Slow", Easing::Spring { mass: 1.0, stiffness: 80.0, damping: 20.0 }),
];

pub fn preset_name(e: &Easing) -> String {
    if let Some((n, _)) = PRESETS.iter().find(|(_, p)| p == e) {
        return (*n).into();
    }
    match e {
        Easing::Bezier(_) => "Custom bezier".into(),
        Easing::Spring { .. } => "Custom spring".into(),
        _ => "Custom".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn endpoints_are_fixed() {
        for (name, e) in PRESETS {
            assert!(close(ease(e, 0.0), 0.0), "{name} start");
            assert!(close(ease(e, 1.0), 1.0), "{name} end");
        }
    }

    #[test]
    fn linear_is_identity_and_clamps() {
        assert!(close(ease(&Easing::Linear, 0.3), 0.3));
        assert!(close(ease(&Easing::Linear, -1.0), 0.0));
        assert!(close(ease(&Easing::Linear, 2.0), 1.0));
    }

    #[test]
    fn standard_curves_have_expected_shape() {
        assert!(ease(&Easing::EaseIn, 0.5) < 0.5);
        assert!(ease(&Easing::EaseOut, 0.5) > 0.5);
        assert!(close(ease(&Easing::EaseInOut, 0.5), 0.5));
        // known css value: ease-in-out(0.25) ~ 0.129
        assert!((ease(&Easing::EaseInOut, 0.25) - 0.129).abs() < 0.01);
    }

    #[test]
    fn curves_are_monotonic_except_back() {
        for e in [Easing::EaseIn, Easing::EaseOut, Easing::EaseInOut, Easing::Bezier([0.1, 0.9, 0.9, 0.1])] {
            let mut prev = 0.0;
            for i in 0..=100 {
                let y = ease(&e, i as f32 / 100.0);
                assert!(y >= prev - 1e-4, "{e:?} at {i}");
                prev = y;
            }
        }
    }

    #[test]
    fn back_curves_overshoot() {
        let min_in = (0..=100).map(|i| ease(&Easing::EaseInBack, i as f32 / 100.0)).fold(f32::MAX, f32::min);
        assert!(min_in < -0.02, "ease in back dips below 0: {min_in}");
        let max_out = (0..=100).map(|i| ease(&Easing::EaseOutBack, i as f32 / 100.0)).fold(f32::MIN, f32::max);
        assert!(max_out > 1.02, "ease out back overshoots 1: {max_out}");
    }

    #[test]
    fn bezier_solver_handles_steep_curves() {
        // x1 = x2 = 0 makes dx/ds zero at s = 0; the solver must still converge.
        let y = cubic_bezier([0.0, 0.0, 0.0, 1.0], 0.001);
        assert!((0.0..=1.0).contains(&y));
        let mid = cubic_bezier([1.0, 0.0, 0.0, 1.0], 0.5);
        assert!(close(mid, 0.5));
    }

    #[test]
    fn bouncy_spring_overshoots_and_gentle_does_not_much() {
        let bouncy = Easing::Spring { mass: 1.0, stiffness: 600.0, damping: 15.0 };
        let max = (0..=200).map(|i| ease(&bouncy, i as f32 / 200.0)).fold(f32::MIN, f32::max);
        assert!(max > 1.05, "bouncy overshoot {max}");
        let overdamped = Easing::Spring { mass: 1.0, stiffness: 100.0, damping: 60.0 };
        let max = (0..=200).map(|i| ease(&overdamped, i as f32 / 200.0)).fold(f32::MIN, f32::max);
        assert!(max <= 1.001, "overdamped never overshoots {max}");
    }

    #[test]
    fn spring_settles_and_ignores_requested_duration() {
        let e = Easing::Spring { mass: 1.0, stiffness: 300.0, damping: 20.0 };
        let secs = spring_settle_secs(1.0, 300.0, 20.0);
        assert!(secs > 0.3 && secs < 1.5, "{secs}");
        assert!(close(spring_at(1.0, 300.0, 20.0, secs), 1.0) || (spring_at(1.0, 300.0, 20.0, secs) - 1.0).abs() < 0.01);
        assert_eq!(effective_duration_ms(&e, 123), (secs * 1000.0) as u32);
        assert_eq!(effective_duration_ms(&Easing::Linear, 123), 123);
    }

    #[test]
    fn critically_damped_is_continuous_with_neighbours() {
        let w0 = 10.0f32;
        let (m, k) = (1.0, w0 * w0);
        let c = 2.0 * (k * m).sqrt();
        let a = spring_at(m, k, c, 0.2);
        let b = spring_at(m, k, c * 0.99, 0.2);
        let d = spring_at(m, k, c * 1.01, 0.2);
        assert!((a - b).abs() < 0.02 && (a - d).abs() < 0.02, "{a} {b} {d}");
    }

    #[test]
    fn preset_names() {
        assert_eq!(preset_name(&Easing::EaseOut), "Ease out");
        assert_eq!(preset_name(&Easing::Bezier([0.1, 0.2, 0.3, 0.4])), "Custom bezier");
        assert_eq!(preset_name(&Easing::Spring { mass: 2.0, stiffness: 5.0, damping: 1.0 }), "Custom spring");
    }
}
