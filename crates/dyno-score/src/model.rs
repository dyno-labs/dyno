//! The memory model and the provisional tier for the reference iPhone. Both are report-only in
//! Phase 1.
//!
//! Stub: the model needs the phone calibration (docs/calibration.md). Until then the model measures and the
//! tier are missing with a launch URL, and unsupported without one.

use dyno_schema::ids::{MeasureId, Phase};
use dyno_schema::measure::{Measure, MeasureStatus, NOT_BUILT_YET};
use dyno_schema::result::{DeviceClass, TierReport};

/// The fitted model's version. None until the phone calibration sets one.
pub const MODEL_VERSION: Option<&str> = None;

/// Every model measure.
pub const MODEL_MEASURES: &[MeasureId] =
    &[MeasureId::ModelFootprint, MeasureId::ModelFootprintInactive];

/// Why there is no footprint or tier without a launch URL.
pub const NEEDS_A_LAUNCH_URL: &str = "needs a launch URL: the footprint adds the JS heap";

/// The plan phase that builds the provisional tier.
const TIER_PHASE: Phase = 1;

/// The modelled footprints. Never required: they decide nothing in Phase 1.
pub fn model_measures(has_launch_url: bool) -> Vec<Measure> {
    let (status, reason) = stub_status(has_launch_url);
    MODEL_MEASURES
        .iter()
        .map(|&id| Measure {
            required: false,
            reason: Some(reason.to_owned()),
            ..Measure::blank(id, status)
        })
        .collect()
}

/// The provisional tier for the reference iPhone.
pub fn provisional_tier(has_launch_url: bool) -> TierReport {
    let (status, reason) = stub_status(has_launch_url);
    TierReport {
        status,
        tier: None,
        device_class: DeviceClass::ReferenceIphone,
        phase: TIER_PHASE,
        reason: Some(reason.to_owned()),
    }
}

/// Without a launch URL there is nothing to model. With one, the model is not built yet.
fn stub_status(has_launch_url: bool) -> (MeasureStatus, &'static str) {
    if has_launch_url {
        (MeasureStatus::Missing, NOT_BUILT_YET)
    } else {
        (MeasureStatus::Unsupported, NEEDS_A_LAUNCH_URL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_measures_are_never_required() {
        for has_launch_url in [false, true] {
            assert!(
                model_measures(has_launch_url)
                    .iter()
                    .all(|measure| !measure.required)
            );
        }
    }

    #[test]
    fn without_a_launch_url_the_model_and_tier_are_unsupported() {
        let measures = model_measures(false);
        assert!(
            measures
                .iter()
                .all(|measure| measure.status == MeasureStatus::Unsupported)
        );
        assert_eq!(provisional_tier(false).status, MeasureStatus::Unsupported);
        assert_eq!(
            provisional_tier(false).reason.as_deref(),
            Some(NEEDS_A_LAUNCH_URL)
        );
    }

    #[test]
    fn with_a_launch_url_the_stub_model_and_tier_are_missing() {
        let measures = model_measures(true);
        assert!(
            measures
                .iter()
                .all(|measure| measure.status == MeasureStatus::Missing)
        );
        assert_eq!(provisional_tier(true).status, MeasureStatus::Missing);
        assert_eq!(provisional_tier(true).tier, None);
    }
}
