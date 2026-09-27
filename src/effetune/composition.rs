//! Pure decisions shared by local playback and clockless remote audio.

pub const MAX_PLUGIN_LATENCY_SECS: f64 = 2.0;

#[derive(Default)]
pub struct StageHealth {
    failures: u32,
    successes: u32,
}

impl StageHealth {
    pub fn failure_count(&self) -> u32 {
        self.failures
    }

    pub fn succeeded(&mut self) {
        self.successes = self.successes.saturating_add(1);
        if self.successes >= 5 {
            self.failures = 0;
        }
    }

    /// Returns true only when the dedicated stage reaches the three-failure threshold.
    pub fn failed(&mut self) -> bool {
        self.successes = 0;
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 3 {
            self.failures = 0;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageComposition {
    pub plugin_latency_secs: f64,
    pub effetune_applied: bool,
    pub limiter_required: bool,
    pub effetune_generation: Option<u64>,
}

/// Called before processing the EffeTune stage. The user chain has priority.
pub fn admit_effetune(user_latency_secs: f64, effetune_latency_secs: f64) -> Result<(), f64> {
    let total = user_latency_secs + effetune_latency_secs;
    if total > MAX_PLUGIN_LATENCY_SECS {
        Err(total)
    } else {
        Ok(())
    }
}

pub fn compose(
    user_applied: bool,
    user_latency_secs: f64,
    effetune_applied: bool,
    effetune_latency_secs: f64,
    effetune_generation: Option<u64>,
) -> StageComposition {
    StageComposition {
        plugin_latency_secs: user_latency_secs
            + if effetune_applied {
                effetune_latency_secs
            } else {
                0.0
            },
        effetune_applied,
        limiter_required: user_applied || effetune_applied,
        effetune_generation: effetune_applied.then_some(effetune_generation).flatten(),
    }
}

/// Select the final samples and stage metadata. On EffeTune failure, `user_samples`
/// is returned unchanged. The other buffer is returned for reuse by the caller.
pub fn compose_samples(
    mut user_samples: Vec<f32>,
    mut effetune_samples: Vec<f32>,
    user_applied: bool,
    user_latency_secs: f64,
    effetune_applied: bool,
    effetune_latency_secs: f64,
    effetune_generation: Option<u64>,
) -> (Vec<f32>, Vec<f32>, StageComposition) {
    if effetune_applied {
        std::mem::swap(&mut user_samples, &mut effetune_samples);
    }
    let composition = compose(
        user_applied,
        user_latency_secs,
        effetune_applied,
        effetune_latency_secs,
        effetune_generation,
    );
    (user_samples, effetune_samples, composition)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_stage_has_priority_and_fallback_keeps_its_latency() {
        assert_eq!(admit_effetune(1.5, 0.5), Ok(()));
        assert_eq!(admit_effetune(1.5, 0.51), Err(2.01));
        let composed = compose(true, 0.3, false, 0.4, Some(8));
        assert_eq!(composed.plugin_latency_secs, 0.3);
        assert!(composed.limiter_required);
        assert_eq!(composed.effetune_generation, None);
        let composed = compose(true, 0.3, true, 0.4, Some(9));
        assert_eq!(composed.plugin_latency_secs, 0.7);
        assert_eq!(composed.effetune_generation, Some(9));
        assert!(compose(false, 0.0, true, 0.0, Some(10)).limiter_required);
    }

    #[test]
    fn effect_health_needs_five_successes_to_clear_partial_failures() {
        let mut health = StageHealth::default();
        assert!(!health.failed());
        for _ in 0..4 {
            health.succeeded();
        }
        assert!(!health.failed());
        assert!(health.failed());
        for _ in 0..5 {
            health.succeeded();
        }
        assert!(!health.failed());
        assert!(!health.failed());
        assert!(health.failed());
    }

    #[test]
    fn failed_effect_returns_the_post_user_chain_samples() {
        let user = vec![0.3, -0.3];
        let effect = vec![0.9, -0.9];
        let (output, scratch, info) =
            compose_samples(user.clone(), effect.clone(), true, 0.2, false, 0.4, Some(4));
        assert_eq!(output, user);
        assert_eq!(scratch, effect);
        assert_eq!(info.plugin_latency_secs, 0.2);
        let (output, scratch, info) =
            compose_samples(user, effect.clone(), true, 0.2, true, 0.4, Some(5));
        assert_eq!(output, effect);
        assert_eq!(scratch, vec![0.3, -0.3]);
        assert_eq!(info.effetune_generation, Some(5));
    }
}
