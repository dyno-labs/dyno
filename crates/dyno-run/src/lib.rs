//! The browser run: load the game in a harness page, wait for idle, and turn what the driver
//! saw into lab measures and contract findings.
//!
//! A [`Driver`] runs the browser. The Rust CDP driver lives in `dyno-cdp`. The other driver is
//! [`exec::ExecDriver`], an outside program such as a Playwright script. Both speak the runner
//! protocol in `dyno_schema::runner`, so the driver choice does not change the result.
//!
//! `lab` turns a run's messages into the load measures and the contract findings. The play
//! measures of the spin loop are Phase 2, so they stay missing.

use std::fmt;

use dyno_schema::ids::{MeasureId, RuleId};
use dyno_schema::measure::{Measure, MeasureStatus, NOT_BUILT_YET, missing_measures};
use dyno_schema::result::DriverInfo;
use dyno_schema::rule::{Finding, incomplete_findings};
use dyno_schema::runner::{DriverMessage, RunRequest};
use dyno_schema::scrub::scrub;

pub mod exec;
pub mod lab;
pub mod protocol;

/// Runs one browser load and reports what it saw.
pub trait Driver {
    /// A short name for errors, such as "cdp".
    fn name(&self) -> &str;

    /// Runs the request and returns every message, in order. The last one is `done` or `error`.
    fn run(&mut self, request: &RunRequest) -> Result<Vec<DriverMessage>, DriverError>;

    /// Whether the driver can send a `heap` reading. A browser that gives none, such as Safari,
    /// has `lab.js_heap` unsupported rather than missing, so the run is not incomplete for it.
    fn reads_heap(&self) -> bool {
        true
    }
}

/// Why a driver run failed: a fixed code, and a detail that has been through [`scrub`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct DriverError {
    code: DriverErrorCode,
    detail: String,
}

/// What kind of failure a driver error is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverErrorCode {
    /// The driver is a stub.
    NotBuilt,
    /// The driver broke the runner protocol.
    Protocol,
    /// The run failed, such as a browser that did not start.
    Run,
}

/// The output of a browser run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabRun {
    /// The driver that ran, as its `hello` names it. None when the run failed.
    pub driver: Option<DriverInfo>,
    /// Every lab and play measure.
    pub measures: Vec<Measure>,
    /// One finding per lab rule.
    pub findings: Vec<Finding>,
}

/// The measures of a load-and-idle run (Phase 1). All are required.
pub const LOAD_MEASURES: &[MeasureId] = &[
    MeasureId::LabTimeFirstFrame,
    MeasureId::LabTimeReady,
    MeasureId::LabBytesLoadedByIdle,
    MeasureId::LabBytesToPlayable,
    MeasureId::LabAudioDecoded,
    MeasureId::LabAudioContextRate,
    MeasureId::LabTexturesUploaded,
    MeasureId::LabTexturesResident,
    MeasureId::LabCanvasContexts,
    MeasureId::LabCanvasBytes,
    MeasureId::LabJsHeap,
    MeasureId::LabInactiveAudioRunning,
    MeasureId::LabInactiveFrameRate,
    MeasureId::LabInactiveAllocated,
    MeasureId::LabSelfCheckFrameDpr,
    MeasureId::LabSelfCheckFrameRequests,
    MeasureId::LabSelfCheckWebglRenderer,
];

/// The measures of the demo spin loop (Phase 2). Required only when `--spins` asks for them.
pub const PLAY_MEASURES: &[MeasureId] = &[
    MeasureId::PlaySpins,
    MeasureId::PlayPeak,
    MeasureId::PlayGrowthPer100Spins,
    MeasureId::PlayBonusPeak,
];

/// The host contract checks and the diagnostic, all from the run.
pub const LAB_RULES: &[RuleId] = &[
    RuleId::ContractReadyInTime,
    RuleId::ContractNoAudioBeforeActive,
    RuleId::ContractThrottledWhileInactive,
    RuleId::DiagnosticAllocBeforeActive,
];

impl DriverError {
    /// An error with its detail scrubbed here. The fields are private, so a driver in another
    /// crate cannot skip the scrub.
    pub fn new(code: DriverErrorCode, detail: &str) -> DriverError {
        DriverError {
            code,
            detail: scrub(detail),
        }
    }

    /// The error of a stub driver.
    pub fn not_built() -> DriverError {
        DriverError::new(DriverErrorCode::NotBuilt, NOT_BUILT_YET)
    }

    /// The fixed code, which needs no scrub.
    pub fn code(&self) -> DriverErrorCode {
        self.code
    }

    /// The scrubbed detail.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl DriverErrorCode {
    /// The code as a reason shows it.
    pub fn as_str(self) -> &'static str {
        match self {
            DriverErrorCode::NotBuilt => "not_built",
            DriverErrorCode::Protocol => "protocol",
            DriverErrorCode::Run => "run",
        }
    }
}

impl fmt::Display for DriverErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Runs the request on the driver and returns the lab measures and findings.
///
/// A run that fails gives every measure as missing and every finding as incomplete, with one
/// reason: "not built yet" for a stub driver, else the error the driver returns or the `error`
/// message it ends its run with. Play measures are Phase 2, so they stay missing, and they are
/// required only when the request asks for spins.
pub fn run_lab(driver: &mut dyn Driver, request: &RunRequest) -> LabRun {
    let messages = match driver.run(request) {
        Ok(messages) => messages,
        Err(error) => return failed_run(driver.name(), request, &error),
    };
    if let Some(error) = final_error(&messages) {
        return failed_run(driver.name(), request, &error);
    }
    let Some((mut measures, findings)) = lab::read(&messages, request) else {
        let error = DriverError::new(DriverErrorCode::Protocol, NEVER_IDLE);
        return failed_run(driver.name(), request, &error);
    };
    if !driver.reads_heap() {
        heap_unsupported(&mut measures);
    }
    measures.extend(play_measures(request, NOT_BUILT_YET));
    LabRun {
        driver: hello_driver(&messages),
        measures,
        findings,
    }
}

/// Why a driver that cannot read the heap has no `lab.js_heap`.
const NO_HEAP: &str = "the browser gives the page no heap reading";

/// `lab.js_heap` unsupported, when it is missing because the driver cannot read the heap.
fn heap_unsupported(measures: &mut [Measure]) {
    for measure in measures
        .iter_mut()
        .filter(|measure| measure.id == MeasureId::LabJsHeap)
        .filter(|measure| measure.status == MeasureStatus::Missing)
    {
        *measure = Measure {
            reason: Some(NO_HEAP.to_owned()),
            ..Measure::blank(MeasureId::LabJsHeap, MeasureStatus::Unsupported)
        };
    }
}

/// Why a run that ended without an `idle` mark has no measures.
const NEVER_IDLE: &str = "the run ended before the game frame went idle";

fn failed_run(driver_name: &str, request: &RunRequest, error: &DriverError) -> LabRun {
    let reason = match error.code() {
        DriverErrorCode::NotBuilt => NOT_BUILT_YET.to_owned(),
        _ => failure_reason(driver_name, error),
    };
    let mut measures = missing_measures(LOAD_MEASURES, &reason);
    measures.extend(play_measures(request, &reason));
    LabRun {
        driver: None,
        measures,
        findings: incomplete_findings(LAB_RULES, &reason),
    }
}

/// The play measures, missing, and required only when the request asks for spins.
fn play_measures(request: &RunRequest, reason: &str) -> Vec<Measure> {
    let spins_asked = request.spins.is_some();
    PLAY_MEASURES
        .iter()
        .map(|&id| Measure {
            required: spins_asked,
            ..Measure::missing(id, reason)
        })
        .collect()
}

/// The driver that the run's `hello` names. Its text is scrubbed, since it comes from outside.
fn hello_driver(messages: &[DriverMessage]) -> Option<DriverInfo> {
    messages.iter().find_map(|message| match message {
        DriverMessage::Hello { driver, .. } => Some(DriverInfo {
            name: scrub(&driver.name),
            version: scrub(&driver.version),
            browser: driver.browser.as_deref().map(scrub),
        }),
        _ => None,
    })
}

/// The driver's own `error` message, which ends a run that failed. The protocol puts it last, so
/// an `error` message earlier in the run is not a failure of the run.
fn final_error(messages: &[DriverMessage]) -> Option<DriverError> {
    match messages.last() {
        Some(DriverMessage::Error { message, .. }) => {
            Some(DriverError::new(DriverErrorCode::Run, message))
        }
        _ => None,
    }
}

/// The reason a failed run gives every lab measure and finding. A driver in another crate names
/// itself, so its name is scrubbed like the detail.
fn failure_reason(driver_name: &str, error: &DriverError) -> String {
    format!(
        "the {} driver failed ({}): {}",
        scrub(driver_name),
        error.code(),
        error.detail()
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use dyno_schema::measure::Source;
    use dyno_schema::profile::{HostProfile, Scenario};
    use dyno_schema::runner::{RUNNER_PROTOCOL_VERSION, RunMark};

    use super::*;

    /// A spin count for a run that asks for the play loop.
    const SPINS: u32 = 100;
    const IDLE_AFTER_MS: u32 = 10_000;
    const TIMEOUT_MS: u32 = 60_000;

    /// A URL with a user name, a password, a token and a fragment, as an error might repeat it.
    const URL_WITH_SECRETS: &str = "https://user:hunter2@games.example/g?token=SECRET#frag";
    /// The parts of `URL_WITH_SECRETS` that no reason may hold.
    const SECRETS: [&str; 4] = ["user", "hunter2", "SECRET", "frag"];

    /// A driver that gives the same outcome on every run.
    struct TestDriver {
        name: String,
        outcome: Result<Vec<DriverMessage>, DriverError>,
        reads_heap: bool,
    }

    impl Driver for TestDriver {
        fn name(&self) -> &str {
            &self.name
        }

        fn run(&mut self, _request: &RunRequest) -> Result<Vec<DriverMessage>, DriverError> {
            self.outcome.clone()
        }

        fn reads_heap(&self) -> bool {
            self.reads_heap
        }
    }

    /// A driver named "failing" that returns an error.
    fn returns_error(code: DriverErrorCode, detail: &str) -> TestDriver {
        TestDriver {
            name: "failing".to_owned(),
            outcome: Err(DriverError::new(code, detail)),
            reads_heap: true,
        }
    }

    /// A driver named "failing" that sends these messages.
    fn sends(messages: Vec<DriverMessage>) -> TestDriver {
        TestDriver {
            name: "failing".to_owned(),
            outcome: Ok(messages),
            reads_heap: true,
        }
    }

    fn error_message(message: &str) -> DriverMessage {
        DriverMessage::Error {
            message: message.to_owned(),
            t_ms: 0.0,
        }
    }

    /// Every reason the run gave, from its measures and findings.
    fn reasons(run: &LabRun) -> Vec<String> {
        let measures = run
            .measures
            .iter()
            .filter_map(|measure| measure.reason.clone());
        let findings = run
            .findings
            .iter()
            .filter_map(|finding| finding.reason.clone());
        measures.chain(findings).collect()
    }

    /// Asserts that every measure and finding of the run gives `reason`.
    fn assert_every_reason_is(run: &LabRun, reason: &str) {
        assert_eq!(
            reasons(run),
            vec![reason; run.measures.len() + run.findings.len()]
        );
    }

    fn request(spins: Option<u32>) -> RunRequest {
        RunRequest {
            protocol_version: RUNNER_PROTOCOL_VERSION,
            launch_url: "https://example.com/game".to_owned(),
            host_profile: HostProfile::reels(),
            scenario: Scenario::Alone,
            spins,
            idle_after_ms: IDLE_AFTER_MS,
            timeout_ms: TIMEOUT_MS,
        }
    }

    /// A driver that is not built yet.
    fn stub_driver() -> TestDriver {
        TestDriver {
            name: "stub".to_owned(),
            outcome: Err(DriverError::not_built()),
            reads_heap: true,
        }
    }

    #[test]
    fn the_load_and_play_lists_cover_every_lab_measure_once() {
        let listed: Vec<MeasureId> = LOAD_MEASURES.iter().chain(PLAY_MEASURES).copied().collect();
        let unique: BTreeSet<&str> = listed.iter().map(|id| id.as_str()).collect();
        let lab: BTreeSet<&str> = MeasureId::ALL
            .iter()
            .filter(|id| id.spec().source == Source::Lab)
            .map(|id| id.as_str())
            .collect();
        assert_eq!(listed.len(), unique.len());
        assert_eq!(unique, lab);
    }

    #[test]
    fn lab_rules_lists_every_lab_rule() {
        let lab: Vec<RuleId> = RuleId::ALL
            .iter()
            .copied()
            .filter(|rule| rule.spec().source == Source::Lab)
            .collect();
        assert_eq!(LAB_RULES, lab);
    }

    #[test]
    fn a_stub_run_gives_every_lab_measure_as_missing_not_built_yet() {
        let run = run_lab(&mut stub_driver(), &request(None));
        assert_eq!(
            run.measures.len(),
            LOAD_MEASURES.len() + PLAY_MEASURES.len()
        );
        for measure in &run.measures {
            assert_eq!(measure.status, MeasureStatus::Missing);
            assert_eq!(measure.reason.as_deref(), Some(NOT_BUILT_YET));
        }
        assert_eq!(run.findings.len(), LAB_RULES.len());
        assert_eq!(run.driver, None);
    }

    #[test]
    fn play_measures_are_required_only_when_spins_are_asked_for() {
        let required = |spins| -> Vec<bool> {
            run_lab(&mut stub_driver(), &request(spins))
                .measures
                .iter()
                .filter(|measure| PLAY_MEASURES.contains(&measure.id))
                .map(|measure| measure.required)
                .collect()
        };
        assert_eq!(required(None), vec![false; PLAY_MEASURES.len()]);
        assert_eq!(required(Some(SPINS)), vec![true; PLAY_MEASURES.len()]);
    }

    /// A run that went idle and sent nothing else, so no heap reading.
    fn went_idle() -> Vec<DriverMessage> {
        vec![
            DriverMessage::Mark {
                mark: RunMark::Idle,
                t_ms: 0.0,
            },
            DriverMessage::Done { t_ms: 0.0 },
        ]
    }

    fn js_heap(run: LabRun) -> Measure {
        run.measures
            .into_iter()
            .find(|measure| measure.id == MeasureId::LabJsHeap)
            .unwrap()
    }

    #[test]
    fn a_driver_that_cannot_read_the_heap_has_it_unsupported_not_missing() {
        let mut safari = TestDriver {
            reads_heap: false,
            ..sends(went_idle())
        };
        let heap = js_heap(run_lab(&mut safari, &request(None)));
        assert_eq!(heap.status, MeasureStatus::Unsupported);
        assert_eq!(heap.reason.as_deref(), Some(NO_HEAP));
        assert!(heap.required);

        let heap = js_heap(run_lab(&mut sends(went_idle()), &request(None)));
        assert_eq!(heap.status, MeasureStatus::Missing);
    }

    #[test]
    fn an_error_the_driver_returns_is_the_reason() {
        let mut driver = returns_error(DriverErrorCode::Run, "no browser");
        let run = run_lab(&mut driver, &request(None));
        assert_every_reason_is(&run, "the failing driver failed (run): no browser");
    }

    #[test]
    fn an_error_message_the_driver_sends_is_the_reason() {
        let mut driver = sends(vec![error_message("the game frame crashed")]);
        let run = run_lab(&mut driver, &request(None));
        assert_every_reason_is(
            &run,
            "the failing driver failed (run): the game frame crashed",
        );
    }

    #[test]
    fn an_error_message_before_the_end_of_the_run_is_not_the_reason() {
        let done = DriverMessage::Done { t_ms: 1.0 };
        let mut driver = sends(vec![error_message("a retry worked"), done]);
        let run = run_lab(&mut driver, &request(None));
        assert_every_reason_is(&run, &never_idle());
    }

    #[test]
    fn a_run_that_never_went_idle_has_no_measures() {
        let mut driver = sends(vec![DriverMessage::Done { t_ms: 1.0 }]);
        let run = run_lab(&mut driver, &request(None));
        assert_every_reason_is(&run, &never_idle());
    }

    fn never_idle() -> String {
        format!("the failing driver failed (protocol): {NEVER_IDLE}")
    }

    #[test]
    fn a_driver_error_detail_is_scrubbed() {
        let error = DriverError::new(
            DriverErrorCode::Protocol,
            &format!("bad line from {URL_WITH_SECRETS}"),
        );
        assert_eq!(error.detail(), "bad line from https://games.example/g");
    }

    #[test]
    fn a_driver_error_shows_its_code_and_detail() {
        let error = DriverError::new(DriverErrorCode::Protocol, "bad line");
        assert_eq!(error.to_string(), "protocol: bad line");
    }

    #[test]
    fn no_secret_in_a_driver_error_reaches_a_reason() {
        let mut drivers = [
            returns_error(DriverErrorCode::Run, URL_WITH_SECRETS),
            sends(vec![error_message(&format!(
                "could not load {URL_WITH_SECRETS}"
            ))]),
            TestDriver {
                name: format!("proxy at {URL_WITH_SECRETS}"),
                ..returns_error(DriverErrorCode::Run, "no browser")
            },
        ];
        for driver in &mut drivers {
            let run = run_lab(driver, &request(None));
            assert!(!reasons(&run).is_empty());
            for reason in reasons(&run) {
                assert!(reason.contains("https://games.example/g"), "{reason}");
                for secret in SECRETS {
                    assert!(!reason.contains(secret), "{secret} in {reason}");
                }
            }
        }
    }
}
