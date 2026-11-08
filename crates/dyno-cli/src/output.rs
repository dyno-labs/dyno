//! Output: canonical JSON to stdout or `--out`, and a short summary for people on stderr.
//!
//! The summary has no stable format. No tool may parse it.

use std::cmp::Reverse;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use dyno_schema::budget::{BYTES_PER_MB, BudgetVerdict};
use dyno_schema::canonical::to_canonical_json;
use dyno_schema::catalogue::CatalogueResult;
use dyno_schema::compare::{AssetChangeKind, CompareResult, MeasureChange};
use dyno_schema::doctor::DoctorReport;
use dyno_schema::ids::{MeasureId, RuleId};
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue, UncertaintyKind, Unit};
use dyno_schema::result::{CheckResult, Deterministic};
use dyno_schema::rule::{Finding, Fix, FixAction};
use dyno_schema::status::{Status, Verdict};
use dyno_score::status::status_of_finding;
use serde::Serialize;

use crate::exit::CliError;

/// Which way a size rounds to a tenth of a MB. A lower bound rounds down and an upper bound
/// rounds up, so each one stays a bound.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rounding {
    Nearest,
    Down,
    Up,
}

/// Measure statuses in the order the summary counts them.
const MEASURE_STATUSES: &[MeasureStatus] = &[
    MeasureStatus::Measured,
    MeasureStatus::Estimated,
    MeasureStatus::Unsupported,
    MeasureStatus::Missing,
    MeasureStatus::Invalid,
];

/// Verdicts in the order the summary counts them.
const VERDICTS: &[Verdict] = &[
    Verdict::Pass,
    Verdict::Warn,
    Verdict::Fail,
    Verdict::Incomplete,
];

/// The memory measures the summary shows, each with its label.
const MEMORY_MEASURES: [(&str, MeasureId); 4] = [
    ("textures decoded", MeasureId::StaticTexturesDecoded),
    ("audio decoded at 48 kHz", MeasureId::StaticAudioDecoded48k),
    ("audio decoded at 24 kHz", MeasureId::StaticAudioDecoded24k),
    ("canvas", MeasureId::StaticRenderCanvas),
];

/// The advice line for antialias, whose saving a device run measures.
const ANTIALIAS_ADVICE: &str = "  turn antialias off, saving not known until a device run";

/// How many fixes the summary shows for each finding, largest saving first.
const FIXES_SHOWN: usize = 3;

/// The last line of a check summary when the JSON went to stdout.
const RESULT_ON_STDOUT: &str =
    "full result: the JSON on stdout. Use --out FILE to write it to a file.";

/// The last line of a check summary in an MCP tool result.
const RESULT_IN_TOOL_RESULT: &str = "full result: the JSON before this summary.";

/// Where the full JSON result is, for the end of a summary.
#[derive(Clone, Copy, Debug)]
pub enum ResultPlace<'a> {
    Stdout,
    /// The file that `--out` named, or the result file that was read.
    File(&'a Path),
    /// The same MCP tool result as the summary.
    ToolResult,
}

impl<'a> ResultPlace<'a> {
    /// Where `--out` puts the result: the file it names, or else stdout.
    pub fn of_out(out: Option<&'a Path>) -> ResultPlace<'a> {
        out.map_or(ResultPlace::Stdout, ResultPlace::File)
    }
}

/// Why a check is incomplete when no measure, budget or rule names the cause.
const NO_ROW: &str = "a required measure has no row";

/// Why a check is invalid when no measure names the cause.
const ROW_DOES_NOT_FIT: &str = "a measure row does not fit the check";

/// Writes `value` as RFC 8785 canonical JSON and a newline, to `out` or else to stdout.
pub fn write_json<T: Serialize>(value: &T, out: Option<&Path>) -> Result<(), CliError> {
    write_text(&json_text(value)?, out)
}

/// `value` as RFC 8785 canonical JSON and a newline: what [`write_json`] writes.
pub fn json_text<T: Serialize>(value: &T) -> Result<String, CliError> {
    let mut json = to_canonical_json(value).map_err(CliError::internal)?;
    json.push('\n');
    Ok(json)
}

/// Writes JSON text to `out` or else to stdout.
pub fn write_text(json: &str, out: Option<&Path>) -> Result<(), CliError> {
    match out {
        Some(path) => fs::write(path, json).map_err(|error| {
            CliError::internal(format!("cannot write {}: {error}", path.display()))
        }),
        None => io::stdout()
            .lock()
            .write_all(json.as_bytes())
            .map_err(|error| CliError::internal(format!("cannot write to stdout: {error}"))),
    }
}

/// Prints a summary line by line to stderr.
pub fn print_summary(lines: &[String]) {
    for line in lines {
        eprintln!("{line}");
    }
}

/// What to do for a served run: open the harness page in the browser to test.
pub fn serve_lines(harness_url: &str, wait: Duration) -> Vec<String> {
    vec![
        format!(
            "dyno check: open {harness_url} in the browser to test, such as Safari on the iPhone."
        ),
        format!(
            "dyno check: the phone must be on this machine's network. Waiting up to {} minutes.",
            wait.as_secs().div_ceil(60)
        ),
    ]
}

/// The status and why, the key numbers, the advice, and where the full result is.
pub fn check_summary(result: &CheckResult, place: ResultPlace) -> Vec<String> {
    let deterministic = &result.deterministic;
    let mut lines = vec![format!(
        "dyno check: {}: {}",
        name(deterministic.status),
        status_reason(deterministic, &result.run.measures)
    )];
    if deterministic.inputs.build {
        lines.push("estimated from the build:".to_owned());
        lines.extend(build_lines(&deterministic.measures));
    }
    if deterministic.inputs.launch_url || deterministic.inputs.device {
        lines.push(format!(
            "run measures: {}",
            count_measures(&result.run.measures)
        ));
    }
    let findings = deterministic.findings.iter().chain(&result.run.findings);
    lines.extend(advice_lines(findings, &deterministic.measures));
    lines.push(budget_line(deterministic));
    lines.push(full_result_line(place));
    lines.push(format!("result_hash: {}", result.result_hash));
    lines
}

pub fn catalogue_summary(catalogue: &CatalogueResult, place: ResultPlace) -> Vec<String> {
    let mut lines = vec![
        format!("dyno check --games: {}", name(catalogue.status)),
        format!("  games checked: {}", catalogue.games.len()),
    ];
    for game in &catalogue.games {
        lines.push(format!(
            "  {}: {}: {}",
            game.id,
            name(game.result.deterministic.status),
            status_reason(&game.result.deterministic, &game.result.run.measures)
        ));
    }
    lines.push(full_result_line(place));
    lines
}

pub fn compare_summary(compare: &CompareResult) -> Vec<String> {
    let deterministic = &compare.deterministic;
    let mut lines = vec![format!("dyno compare: {}", name(deterministic.status))];
    if let Some(reason) = &deterministic.reason {
        lines.push(format!("  {reason}"));
    }
    for change in &deterministic.verdicts {
        lines.push(format!(
            "  budget {}: {} to {}",
            change.budget,
            verdict_name(change.base),
            verdict_name(change.head)
        ));
    }
    let moved: Vec<&MeasureChange> = deterministic
        .measures
        .iter()
        .filter(|change| change.delta.is_some_and(|delta| delta != 0))
        .collect();
    for change in moved.iter().take(MEASURES_SHOWN) {
        lines.push(measure_change_line(change));
    }
    let more = moved.len().saturating_sub(MEASURES_SHOWN);
    if more > 0 {
        lines.push(format!(
            "  and {}",
            count(more, "more measure", "more measures")
        ));
    }
    let assets = &deterministic.suspect_assets;
    if !assets.is_empty() {
        let of_kind = |kind| assets.iter().filter(|asset| asset.change == kind).count();
        lines.push(format!(
            "  files: {} added, {} removed, {} changed",
            of_kind(AssetChangeKind::Added),
            of_kind(AssetChangeKind::Removed),
            of_kind(AssetChangeKind::Changed)
        ));
    }
    lines
}

/// How many moved measures the compare summary shows.
const MEASURES_SHOWN: usize = 8;

fn verdict_name(verdict: Option<Verdict>) -> &'static str {
    match verdict {
        Some(Verdict::Pass) => "pass",
        Some(Verdict::Warn) => "warn",
        Some(Verdict::Fail) => "fail",
        Some(Verdict::Incomplete) => "incomplete",
        None => "no verdict",
    }
}

/// "  static.audio.decoded_48k: 120.7 MB to 60.3 MB (-60.4 MB)", in MB for bytes.
fn measure_change_line(change: &MeasureChange) -> String {
    let (Some(MeasureValue::Integer(base)), Some(MeasureValue::Integer(head)), Some(delta)) =
        (&change.base, &change.head, change.delta)
    else {
        return format!("  {}: changed", change.id);
    };
    let shown = |value: i64| match change.unit {
        Unit::Bytes => signed_mb(value),
        _ => value.to_string(),
    };
    let sign = if delta > 0 { "+" } else { "" };
    format!(
        "  {}: {} to {} ({sign}{})",
        change.id,
        shown(*base),
        shown(*head),
        shown(delta)
    )
}

/// Bytes in MB with one decimal, negative values included.
fn signed_mb(bytes: i64) -> String {
    let magnitude = mb(bytes.unsigned_abs(), Rounding::Nearest);
    if bytes < 0 {
        format!("-{magnitude}")
    } else {
        magnitude
    }
}

pub fn doctor_summary(report: &DoctorReport) -> Vec<String> {
    vec![format!(
        "dyno doctor: {}, a Phase {} command",
        name(report.status),
        report.phase
    )]
}

/// Why the check has its status, in a few words.
fn status_reason(deterministic: &Deterministic, run_measures: &[Measure]) -> String {
    match deterministic.status {
        Status::Pass => "nothing warns or fails".to_owned(),
        Status::Warn => budgets_and_rules(deterministic, Verdict::Warn),
        Status::Fail => budgets_and_rules(deterministic, Verdict::Fail),
        Status::Incomplete => first_cause(&incomplete_causes(deterministic, run_measures), NO_ROW),
        Status::Invalid => first_cause(
            &invalid_causes(deterministic, run_measures),
            ROW_DOES_NOT_FIT,
        ),
    }
}

/// "1 budget and 2 rules warn": how many budgets and static rules give the verdict.
fn budgets_and_rules(deterministic: &Deterministic, verdict: Verdict) -> String {
    let budgets = deterministic
        .verdicts
        .iter()
        .filter(|budget| budget.verdict == verdict)
        .count();
    let rules = deterministic
        .findings
        .iter()
        .filter(|finding| finding.verdict == verdict)
        .count();
    let subject = match (budgets, rules) {
        (0, rules) => count(rules, "rule", "rules"),
        (budgets, 0) => count(budgets, "budget", "budgets"),
        (budgets, rules) => format!(
            "{} and {}",
            count(budgets, "budget", "budgets"),
            count(rules, "rule", "rules")
        ),
    };
    let verb = name(verdict);
    if budgets + rules == 1 {
        format!("{subject} {verb}s")
    } else {
        format!("{subject} {verb}")
    }
}

/// What makes a check incomplete: required measures that are missing, then budgets and static
/// rules that could not be checked.
fn incomplete_causes(deterministic: &Deterministic, run_measures: &[Measure]) -> Vec<String> {
    let budgets = deterministic
        .verdicts
        .iter()
        .filter(|budget| budget.verdict == Verdict::Incomplete)
        .map(|budget| format!("budget {} could not be checked", budget.budget));
    let rules = deterministic
        .findings
        .iter()
        .filter(|finding| status_of_finding(finding, &deterministic.measures) == Status::Incomplete)
        .map(|finding| {
            let cause = format!("rule {} could not be checked", finding.rule);
            with_reason(cause, finding.reason.as_deref())
        });
    required_measures_that_are(MeasureStatus::Missing, deterministic, run_measures)
        .chain(budgets)
        .chain(rules)
        .collect()
}

fn invalid_causes(deterministic: &Deterministic, run_measures: &[Measure]) -> Vec<String> {
    required_measures_that_are(MeasureStatus::Invalid, deterministic, run_measures).collect()
}

/// "static.render.backend is missing (no render config found)" for each required measure with
/// the status, static measures first.
fn required_measures_that_are<'a>(
    status: MeasureStatus,
    deterministic: &'a Deterministic,
    run_measures: &'a [Measure],
) -> impl Iterator<Item = String> + 'a {
    deterministic
        .measures
        .iter()
        .chain(run_measures)
        .filter(move |measure| measure.required && measure.status == status)
        .map(|measure| format!("{} is {}", measure.id, value_text(Some(measure))))
}

/// The first cause, and how many more there are. The fallback when there is none.
fn first_cause(causes: &[String], fallback: &str) -> String {
    match causes {
        [] => fallback.to_owned(),
        [only] => only.clone(),
        [first, more @ ..] => format!(
            "{first}, and {}",
            count(more.len(), "more cause", "more causes")
        ),
    }
}

/// The key memory numbers, then the files and their size.
fn build_lines(measures: &[Measure]) -> Vec<String> {
    let value_of = |id| value_text(measures.iter().find(|measure| measure.id == id));
    let mut lines: Vec<String> = MEMORY_MEASURES
        .iter()
        .map(|&(label, id)| format!("  {label}: {}", value_of(id)))
        .collect();
    lines.push(format!(
        "  files: {}, build size: {}, compressed text: {}",
        value_of(MeasureId::StaticInventoryFiles),
        value_of(MeasureId::StaticBytesByType),
        value_of(MeasureId::StaticBytesCompressed)
    ));
    lines
}

/// A measure's value, as a floor or a range when its uncertainty says so. Its status and reason
/// when it has no value.
fn value_text(measure: Option<&Measure>) -> String {
    let Some(measure) = measure else {
        return "not in the result".to_owned();
    };
    match (measure.status, &measure.value) {
        (
            MeasureStatus::Measured | MeasureStatus::Estimated,
            Some(MeasureValue::Integer(number)),
        ) => number_text(measure, *number),
        (status, _) => with_reason(name(status), measure.reason.as_deref()),
    }
}

/// "at least 12.3 MB" for a floor, "12.3 MB to 24.6 MB" for a range, and "12.3 MB" otherwise. A
/// range whose two ends give the same text gives it once.
fn number_text(measure: &Measure, number: i64) -> String {
    let uncertainty = &measure.uncertainty;
    let amount = |number, rounding| amount(measure.unit, number, rounding);
    match (uncertainty.kind, uncertainty.low, uncertainty.high) {
        (UncertaintyKind::Range, Some(low), Some(high)) => {
            let low = amount(low, Rounding::Down);
            let high = amount(high, Rounding::Up);
            if low == high {
                low
            } else {
                format!("{low} to {high}")
            }
        }
        (UncertaintyKind::Floor, _, _) => format!("at least {}", amount(number, Rounding::Down)),
        _ => amount(number, Rounding::Nearest),
    }
}

/// MB for a size, and the plain number for anything else.
fn amount(unit: Unit, number: i64, rounding: Rounding) -> String {
    match (unit, u64::try_from(number)) {
        (Unit::Bytes, Ok(bytes)) => mb(bytes, rounding),
        _ => number.to_string(),
    }
}

/// Bytes as MB with one decimal, such as "12.3 MB". A size above 0 that rounds to the nearest
/// 0.0 is "under 0.1 MB".
fn mb(bytes: u64, rounding: Rounding) -> String {
    let tenths = bytes.saturating_mul(10);
    let rounded = match rounding {
        Rounding::Nearest => tenths.saturating_add(BYTES_PER_MB / 2) / BYTES_PER_MB,
        Rounding::Down => tenths / BYTES_PER_MB,
        Rounding::Up => tenths.div_ceil(BYTES_PER_MB),
    };
    if rounding == Rounding::Nearest && rounded == 0 && bytes > 0 {
        return "under 0.1 MB".to_owned();
    }
    format!("{}.{} MB", rounded / 10, rounded % 10)
}

/// Each finding that warns or fails, fails first, with its largest fixes. `measures` are the
/// static measures, for advice that has no fix yet.
fn advice_lines<'a>(
    findings: impl Iterator<Item = &'a Finding>,
    measures: &[Measure],
) -> Vec<String> {
    let mut flagged: Vec<&Finding> = findings
        .filter(|finding| matches!(finding.verdict, Verdict::Warn | Verdict::Fail))
        .collect();
    // False sorts first, so fails come first. The sort is stable, so each part keeps its order.
    flagged.sort_by_key(|finding| finding.verdict != Verdict::Fail);
    flagged
        .into_iter()
        .flat_map(|finding| finding_lines(finding, measures))
        .collect()
}

/// "warn audio.stereo: stereo sounds can be mono", then its largest fixes, then advice with no
/// fix yet.
fn finding_lines(finding: &Finding, measures: &[Measure]) -> Vec<String> {
    let mut lines = vec![format!(
        "{} {}: {}",
        name(finding.verdict),
        finding.rule,
        rule_message(finding.rule)
    )];
    let mut fixes: Vec<&Fix> = finding.fixes.iter().collect();
    // The fixes are sorted by path and the sort is stable, so ties stay in path order.
    fixes.sort_by_key(|fix| Reverse((memory_saving(fix), fix.saves_bytes)));
    lines.extend(fixes.iter().take(FIXES_SHOWN).map(|fix| fix_line(fix)));
    let more = fixes.len().saturating_sub(FIXES_SHOWN);
    if more > 0 {
        lines.push(format!("  and {}", count(more, "more fix", "more fixes")));
    }
    if needs_antialias_advice(finding, measures) {
        lines.push(ANTIALIAS_ADVICE.to_owned());
    }
    lines
}

/// Whether the render rule warns with antialias on but has no fix to turn it off: its saving
/// waits for a device run.
fn needs_antialias_advice(finding: &Finding, measures: &[Measure]) -> bool {
    let antialias_on = measures.iter().any(|measure| {
        measure.id == MeasureId::StaticRenderAntialias
            && measure.value == Some(MeasureValue::Flag(true))
    });
    let has_fix = finding
        .fixes
        .iter()
        .any(|fix| fix.action == FixAction::TurnOffAntialias);
    finding.rule == RuleId::RenderAntialiasOrResolution && antialias_on && !has_fix
}

/// What a rule that warns or fails says, in a few words.
fn rule_message(rule: RuleId) -> &'static str {
    match rule {
        RuleId::AudioPcmOverBudget => "decoded audio is above its limit",
        RuleId::AudioStereo => "stereo sounds can be mono",
        RuleId::AudioSilenceShare => "sprite maps hold too much silence",
        RuleId::RenderAntialiasOrResolution => "antialias is on, or the resolution is not capped",
        RuleId::TexturesOverBudget => "decoded textures are above their limit",
        RuleId::FilesDuplicate => "some files are copies of other files",
        RuleId::FilesUnreferenced => "nothing uses some files",
        RuleId::ContractReadyInTime => "the ready message came late, or did not come",
        RuleId::ContractNoAudioBeforeActive => "audio ran before the game was active",
        RuleId::ContractThrottledWhileInactive => {
            "the game rendered more than 1 frame a second while inactive"
        }
        RuleId::DiagnosticAllocBeforeActive => {
            "the game decoded audio or uploaded textures before it was active"
        }
    }
}

/// The decoded bytes a fix saves: `saves_decoded_bytes` when it is set, because `saves_bytes`
/// then counts download bytes.
fn memory_saving(fix: &Fix) -> u64 {
    fix.saves_decoded_bytes.unwrap_or(fix.saves_bytes)
}

/// "  audio/a.ogg: mix down to mono, saves 0.5 MB". A copy also gives its download saving.
fn fix_line(fix: &Fix) -> String {
    let saving = match fix.saves_decoded_bytes {
        Some(decoded) => format!(
            "saves {} to download and {} decoded",
            mb(fix.saves_bytes, Rounding::Nearest),
            mb(decoded, Rounding::Nearest)
        ),
        None => format!("saves {}", mb(fix.saves_bytes, Rounding::Nearest)),
    };
    let action = action_words(fix.action);
    match (&fix.path, fix.action) {
        (Some(path), FixAction::KeepOneCopy) => {
            format!("  {path} and its copies: {action}, {saving}")
        }
        (Some(path), _) => format!("  {path}: {action}, {saving}"),
        (None, _) => format!("  {action}, {saving}"),
    }
}

/// What a fix asks for, in a few words.
fn action_words(action: FixAction) -> &'static str {
    match action {
        FixAction::CapResolution => "cap the resolution",
        FixAction::DownmixToMono => "mix down to mono",
        FixAction::KeepOneCopy => "keep only the one the game loads",
        FixAction::RemoveDuplicate => "delete this copy",
        FixAction::ResampleTo24k => "play the audio at 24 kHz",
        FixAction::ShrinkTextures => "shrink or GPU-compress",
        FixAction::TrimSpriteSilence => "cut the silence",
        FixAction::TurnOffAntialias => "turn antialias off",
    }
}

fn budget_line(deterministic: &Deterministic) -> String {
    let budgets = if deterministic.inputs.budget.is_some() {
        count_verdicts(&deterministic.verdicts).unwrap_or_else(|| "none set".to_owned())
    } else {
        "no budget file, so the rules use their default limits".to_owned()
    };
    format!("budgets: {budgets}")
}

fn full_result_line(place: ResultPlace) -> String {
    match place {
        ResultPlace::Stdout => RESULT_ON_STDOUT.to_owned(),
        ResultPlace::File(path) => format!("full result: {}", path.display()),
        ResultPlace::ToolResult => RESULT_IN_TOOL_RESULT.to_owned(),
    }
}

fn count_measures(measures: &[Measure]) -> String {
    counts(MEASURE_STATUSES, |status| {
        measures
            .iter()
            .filter(|measure| measure.status == *status)
            .count()
    })
    .unwrap_or_else(|| "none".to_owned())
}

/// The verdict counts. None when there are no verdicts.
fn count_verdicts(verdicts: &[BudgetVerdict]) -> Option<String> {
    counts(VERDICTS, |verdict| {
        verdicts
            .iter()
            .filter(|candidate| candidate.verdict == *verdict)
            .count()
    })
}

/// "3 estimated, 14 missing": each non-zero count with its name. None when every count is 0.
fn counts<T: Serialize>(kinds: &[T], count_of: impl Fn(&T) -> usize) -> Option<String> {
    let parts: Vec<String> = kinds
        .iter()
        .map(|kind| (count_of(kind), kind))
        .filter(|(count, _)| *count > 0)
        .map(|(count, kind)| format!("{count} {}", name(kind)))
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// "1 rule" or "2 rules".
fn count(amount: usize, one: &str, many: &str) -> String {
    if amount == 1 {
        format!("1 {one}")
    } else {
        format!("{amount} {many}")
    }
}

/// "missing (not built yet)", or the text alone when there is no reason.
fn with_reason(text: String, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("{text} ({reason})"),
        None => text,
    }
}

/// The JSON name of a unit enum, such as "incomplete".
fn name<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "?".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dyno_schema::budget::BudgetLimit;
    use dyno_schema::ids::BudgetId;
    use dyno_schema::measure::Uncertainty;
    use dyno_schema::path::RelPath;
    use dyno_schema::profile::{HostProfile, Scenario};
    use dyno_schema::result::{Inputs, Versions};
    use dyno_schema::status::RunCoverage;

    use super::*;

    const MB: u64 = BYTES_PER_MB;

    /// The fewest bytes that round up to 0.1 MB, which is half of a tenth of a MB.
    const SMALLEST_TENTH: u64 = MB.div_ceil(20);

    /// A build-only check with this status and nothing in it yet.
    fn a_check(status: Status) -> Deterministic {
        Deterministic {
            versions: Versions {
                schema: 0,
                tool: "0.0.0".to_owned(),
                scoring: 0,
                model: None,
            },
            inputs: Inputs {
                build: true,
                launch_url: false,
                host_profile: HostProfile::reels().reference(),
                budget: None,
                scenario: Scenario::Alone,
                spins: None,
                device: false,
            },
            build: None,
            status,
            run_coverage: RunCoverage::NotRequested,
            measures: Vec::new(),
            verdicts: Vec::new(),
            findings: Vec::new(),
        }
    }

    fn budget(verdict: Verdict) -> BudgetVerdict {
        BudgetVerdict {
            budget: BudgetId::AudioDecodedMb,
            measure: MeasureId::StaticAudioDecoded48k,
            verdict,
            actual: None,
            limit: BudgetLimit::Bytes {
                warn_above: None,
                fail_above: Some(1),
            },
        }
    }

    fn finding(verdict: Verdict, fixes: Vec<Fix>) -> Finding {
        Finding {
            verdict,
            fixes,
            reason: None,
            ..Finding::incomplete(RuleId::AudioStereo, "")
        }
    }

    fn fix(path: &str, saves_bytes: u64) -> Fix {
        Fix {
            action: FixAction::DownmixToMono,
            path: Some(RelPath::new(path).unwrap()),
            saves_bytes,
            saves_decoded_bytes: None,
        }
    }

    fn estimated_bytes(bytes: u64, uncertainty: Uncertainty) -> Measure {
        Measure {
            value: Some(MeasureValue::Integer(bytes as i64)),
            uncertainty,
            ..Measure::blank(MeasureId::StaticTexturesDecoded, MeasureStatus::Estimated)
        }
    }

    fn uncertainty(kind: UncertaintyKind, low: Option<u64>, high: Option<u64>) -> Uncertainty {
        Uncertainty {
            kind,
            low: low.map(|low| low as i64),
            high: high.map(|high| high as i64),
            note: None,
        }
    }

    fn reason_with(budgets: Vec<BudgetVerdict>, findings: Vec<Finding>, status: Status) -> String {
        let check = Deterministic {
            verdicts: budgets,
            findings,
            ..a_check(status)
        };
        status_reason(&check, &[])
    }

    #[test]
    fn sizes_are_rounded_to_the_nearest_tenth_of_a_mb() {
        let nearest = |bytes| mb(bytes, Rounding::Nearest);
        assert_eq!(nearest(0), "0.0 MB");
        assert_eq!(nearest(SMALLEST_TENTH), "0.1 MB");
        assert_eq!(nearest(3 * MB / 2), "1.5 MB");
        assert_eq!(nearest(10 * MB - 1), "10.0 MB");
    }

    #[test]
    fn a_size_that_rounds_to_zero_is_under_a_tenth_of_a_mb() {
        assert_eq!(mb(1, Rounding::Nearest), "under 0.1 MB");
        assert_eq!(mb(SMALLEST_TENTH - 1, Rounding::Nearest), "under 0.1 MB");
    }

    #[test]
    fn a_floor_is_at_least_its_value() {
        let floor = estimated_bytes(MB, uncertainty(UncertaintyKind::Floor, None, None));
        assert_eq!(value_text(Some(&floor)), "at least 1.0 MB");
    }

    #[test]
    fn a_floor_rounds_down_so_it_stays_a_floor() {
        let floor = uncertainty(UncertaintyKind::Floor, None, None);
        let nearly_two = estimated_bytes(2 * MB - 1, floor.clone());
        assert_eq!(value_text(Some(&nearly_two)), "at least 1.9 MB");
        let tiny = estimated_bytes(1, floor);
        assert_eq!(value_text(Some(&tiny)), "at least 0.0 MB");
    }

    #[test]
    fn a_range_gives_its_low_and_high_ends() {
        let range = uncertainty(UncertaintyKind::Range, Some(MB), Some(2 * MB));
        assert_eq!(
            value_text(Some(&estimated_bytes(MB, range))),
            "1.0 MB to 2.0 MB"
        );
    }

    #[test]
    fn a_range_rounds_its_low_end_down_and_its_high_end_up() {
        let range = uncertainty(UncertaintyKind::Range, Some(2 * MB - 1), Some(2 * MB + 1));
        assert_eq!(
            value_text(Some(&estimated_bytes(2 * MB, range))),
            "1.9 MB to 2.1 MB"
        );
    }

    #[test]
    fn a_range_whose_ends_give_the_same_text_gives_it_once() {
        let range = uncertainty(UncertaintyKind::Range, Some(0), Some(0));
        assert_eq!(value_text(Some(&estimated_bytes(0, range))), "0.0 MB");
    }

    #[test]
    fn a_range_without_both_ends_gives_the_value() {
        let open_range = uncertainty(UncertaintyKind::Range, Some(MB), None);
        assert_eq!(
            value_text(Some(&estimated_bytes(3 * MB, open_range))),
            "3.0 MB"
        );
    }

    #[test]
    fn a_count_is_a_plain_number() {
        let files = Measure {
            value: Some(MeasureValue::Integer(12)),
            ..Measure::blank(MeasureId::StaticInventoryFiles, MeasureStatus::Estimated)
        };
        assert_eq!(value_text(Some(&files)), "12");
    }

    #[test]
    fn a_measure_without_a_value_gives_its_status_and_reason() {
        let missing = Measure::missing(MeasureId::StaticRenderCanvas, "no render config");
        assert_eq!(value_text(Some(&missing)), "missing (no render config)");
        let invalid = Measure::blank(MeasureId::StaticRenderCanvas, MeasureStatus::Invalid);
        assert_eq!(value_text(Some(&invalid)), "invalid");
    }

    #[test]
    fn a_measure_with_no_row_is_not_in_the_result() {
        assert_eq!(value_text(None), "not in the result");
    }

    #[test]
    fn a_warn_counts_the_budgets_and_rules_that_warn() {
        let warn = Verdict::Warn;
        let cases = [
            (vec![budget(warn)], vec![], "1 budget warns"),
            (vec![], vec![finding(warn, vec![])], "1 rule warns"),
            (vec![budget(warn), budget(warn)], vec![], "2 budgets warn"),
            (
                vec![budget(warn), budget(Verdict::Pass)],
                vec![finding(warn, vec![]), finding(warn, vec![])],
                "1 budget and 2 rules warn",
            ),
        ];
        for (budgets, findings, reason) in cases {
            assert_eq!(reason_with(budgets, findings, Status::Warn), reason);
        }
    }

    #[test]
    fn a_fail_counts_only_what_fails() {
        let findings = vec![
            finding(Verdict::Fail, vec![]),
            finding(Verdict::Warn, vec![]),
        ];
        assert_eq!(
            reason_with(vec![budget(Verdict::Warn)], findings, Status::Fail),
            "1 rule fails"
        );
    }

    #[test]
    fn a_budget_that_could_not_be_checked_makes_a_cause() {
        assert_eq!(
            reason_with(
                vec![budget(Verdict::Incomplete)],
                vec![],
                Status::Incomplete
            ),
            "budget audio.decoded_mb could not be checked"
        );
    }

    #[test]
    fn several_causes_give_the_first_and_how_many_more() {
        let budgets = vec![budget(Verdict::Incomplete), budget(Verdict::Incomplete)];
        assert_eq!(
            reason_with(budgets, vec![], Status::Incomplete),
            "budget audio.decoded_mb could not be checked, and 1 more cause"
        );
    }

    #[test]
    fn an_incomplete_check_with_no_named_cause_says_a_row_is_missing() {
        assert_eq!(reason_with(vec![], vec![], Status::Incomplete), NO_ROW);
    }

    #[test]
    fn a_missing_run_measure_makes_a_cause() {
        let heap = Measure::missing(MeasureId::LabJsHeap, "not built yet");
        assert_eq!(
            status_reason(&a_check(Status::Incomplete), &[heap]),
            "lab.js_heap is missing (not built yet)"
        );
    }

    #[test]
    fn an_optional_missing_measure_is_not_a_cause() {
        let source_map = Measure {
            required: false,
            ..Measure::missing(MeasureId::StaticBundleSourceMap, "not built yet")
        };
        let check = Deterministic {
            measures: vec![source_map],
            ..a_check(Status::Incomplete)
        };
        assert_eq!(status_reason(&check, &[]), NO_ROW);
    }

    #[test]
    fn an_invalid_check_names_the_invalid_measure() {
        let canvas = Measure {
            reason: Some("negative".to_owned()),
            ..Measure::blank(MeasureId::StaticRenderCanvas, MeasureStatus::Invalid)
        };
        let check = Deterministic {
            measures: vec![canvas],
            ..a_check(Status::Invalid)
        };
        assert_eq!(
            status_reason(&check, &[]),
            "static.render.canvas is invalid (negative)"
        );
    }

    #[test]
    fn an_invalid_check_with_no_invalid_measure_says_a_row_does_not_fit() {
        assert_eq!(
            status_reason(&a_check(Status::Invalid), &[]),
            ROW_DOES_NOT_FIT
        );
    }

    #[test]
    fn fixes_show_the_largest_memory_saving_first() {
        let fixes = vec![
            fix("a.ogg", MB),
            fix("b.ogg", 4 * MB),
            fix("c.ogg", 2 * MB),
            fix("d.ogg", 3 * MB),
        ];
        assert_eq!(
            finding_lines(&finding(Verdict::Warn, fixes), &[]),
            [
                "warn audio.stereo: stereo sounds can be mono",
                "  b.ogg: mix down to mono, saves 4.0 MB",
                "  d.ogg: mix down to mono, saves 3.0 MB",
                "  c.ogg: mix down to mono, saves 2.0 MB",
                "  and 1 more fix",
            ]
        );
    }

    #[test]
    fn a_copy_is_ranked_by_its_decoded_saving() {
        let copy = |path: &str, download: u64, decoded: u64| Fix {
            action: FixAction::RemoveDuplicate,
            saves_decoded_bytes: Some(decoded),
            ..fix(path, download)
        };
        let fixes = vec![copy("a.png", 2 * MB, 0), copy("b.png", MB, 8 * MB)];
        let lines = finding_lines(&finding(Verdict::Warn, fixes), &[]);
        assert_eq!(
            lines[1],
            "  b.png: delete this copy, saves 1.0 MB to download and 8.0 MB decoded"
        );
    }

    #[test]
    fn fails_come_before_warns() {
        let warn = Finding {
            rule: RuleId::AudioPcmOverBudget,
            ..finding(Verdict::Warn, vec![])
        };
        let fail = finding(Verdict::Fail, vec![]);
        let lines = advice_lines([&warn, &fail].into_iter(), &[]);
        assert_eq!(
            lines,
            [
                "fail audio.stereo: stereo sounds can be mono",
                "warn audio.pcm_over_budget: decoded audio is above its limit",
            ]
        );
    }

    #[test]
    fn antialias_on_with_no_fix_gives_advice_without_a_saving() {
        let render = Finding {
            rule: RuleId::RenderAntialiasOrResolution,
            ..finding(Verdict::Warn, vec![])
        };
        let antialias = Measure {
            value: Some(MeasureValue::Flag(true)),
            ..Measure::blank(MeasureId::StaticRenderAntialias, MeasureStatus::Estimated)
        };
        assert_eq!(
            finding_lines(&render, std::slice::from_ref(&antialias)),
            [
                "warn render.antialias_or_resolution: antialias is on, or the resolution is not capped",
                ANTIALIAS_ADVICE,
            ]
        );
        let off = Measure {
            value: Some(MeasureValue::Flag(false)),
            ..antialias
        };
        assert_eq!(finding_lines(&render, &[off]).len(), 1);
    }

    #[test]
    fn findings_that_pass_or_are_incomplete_give_no_advice() {
        let pass = finding(Verdict::Pass, vec![fix("a.ogg", MB)]);
        let incomplete = Finding::incomplete(RuleId::AudioStereo, "no estimate");
        assert!(advice_lines([&pass, &incomplete].into_iter(), &[]).is_empty());
    }

    #[test]
    fn the_full_result_line_names_the_out_file() {
        let out = PathBuf::from("results/game.json");
        assert_eq!(
            full_result_line(ResultPlace::File(&out)),
            "full result: results/game.json"
        );
    }
}
