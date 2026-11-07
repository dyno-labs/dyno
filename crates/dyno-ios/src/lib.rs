//! The iPhone lab (Phase 2): `dyno doctor` and `dyno check --device`.
//!
//! It drives Apple's command line tools as subprocesses: `xcrun xctrace` for `phys_footprint`,
//! `devicectl` for control, and the syslog for kills.
//!
//! Stub: it runs no command and touches no device.

use dyno_schema::SCHEMA_VERSION;
use dyno_schema::doctor::{DoctorCheck, DoctorCheckId, DoctorReport};
use dyno_schema::ids::{MeasureId, Phase};
use dyno_schema::measure::{Measure, NOT_BUILT_YET};
use dyno_schema::status::{Status, Verdict};

/// The plan phase that builds `doctor` and the device run.
pub const DEVICE_PHASE: Phase = 2;

/// The measures of a device run. A check has them only when `--device` asks for them, so they
/// are always required.
pub const DEVICE_MEASURES: &[MeasureId] = &[MeasureId::DevicePhysFootprint, MeasureId::DeviceKills];

/// Whether this Mac can run the game on an iPhone.
///
/// Stub: every check is incomplete, and the reason names the phase. The built version checks
/// Xcode, a connected iPhone and trust.
pub fn doctor() -> DoctorReport {
    let checks = DoctorCheckId::ALL
        .iter()
        .map(|&id| DoctorCheck {
            id,
            verdict: Verdict::Incomplete,
            phase: DEVICE_PHASE,
            detail: None,
            reason: Some(format!(
                "{NOT_BUILT_YET}: doctor is a Phase {DEVICE_PHASE} command"
            )),
        })
        .collect();
    DoctorReport {
        schema_version: SCHEMA_VERSION,
        phase: DEVICE_PHASE,
        status: Status::Incomplete,
        checks,
    }
}

/// The device measures of a run on the connected iPhone.
///
/// Stub: every measure is missing with the reason "not built yet". The built version runs the
/// game in fresh Safari processes and reads `phys_footprint` and kills.
pub fn device_measures() -> Vec<Measure> {
    DEVICE_MEASURES
        .iter()
        .map(|&id| Measure {
            required: true,
            ..Measure::missing(id, NOT_BUILT_YET)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use dyno_schema::measure::{MeasureStatus, Source};

    use super::*;

    #[test]
    fn a_stub_doctor_is_incomplete_and_names_its_phase() {
        let report = doctor();
        assert_eq!(report.status, Status::Incomplete);
        assert_eq!(report.phase, 2);
        assert_eq!(report.checks.len(), DoctorCheckId::ALL.len());
        for check in &report.checks {
            assert_eq!(check.verdict, Verdict::Incomplete);
            assert!(check.reason.as_deref().unwrap().contains("Phase 2"));
        }
    }

    #[test]
    fn device_measures_cover_every_device_measure_and_are_required() {
        let measures = device_measures();
        let device: Vec<MeasureId> = MeasureId::ALL
            .iter()
            .copied()
            .filter(|id| id.spec().source == Source::Device)
            .collect();
        let ids: Vec<MeasureId> = measures.iter().map(|measure| measure.id).collect();
        assert_eq!(ids, device);
        for measure in &measures {
            assert!(measure.required);
            assert_eq!(measure.status, MeasureStatus::Missing);
        }
    }
}
