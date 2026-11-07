//! The phone calibration of the memory model, fixed before any phone run.
//! `docs/calibration.md` is the run sheet.
//!
//! The model: footprint = decoded audio + GPU textures + canvas buffers + k x JS
//! heap + b, with k and b fitted to `phys_footprint` on the reference iPhone. Decoded images
//! that are not textures are not measured yet, so they are not a part. This module holds only
//! the method: the phone value, the fit, the signed error and the gate. It reads no device; the
//! device thread records the runs on the reference phone in a calibration file.
//!
//! Two recipes give the parts, so the scoring owner can compare them on the same runs before
//! choosing one for the model version: `static`, with the parts from the build, and `lab`, with
//! the parts from the browser run. Both take the JS heap from the browser run.
//!
//! A game enters the fit and the gate only when its phone value is sound: not blocked by kills,
//! with at least one run, and above the baseline. Every other game is still reported, with why
//! it was left out.

use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue};
use dyno_schema::result::CheckResult;
use serde::{Deserialize, Serialize};

/// The worst held-out underprediction at which the total may still feed the provisional tier
/// under 20%.
pub const GATE_UNDERPREDICTION: f64 = 0.20;

/// The `playable` line while active, before headroom: 500 MB. A placeholder: the calibration
/// sets it.
pub const PLAYABLE_LINE_BYTES: u64 = 500 * 1024 * 1024;

/// Fresh runs per game, games to fit and games to hold out. The gate needs all the
/// held-out games.
pub const RUNS_PER_GAME: usize = 3;
pub const FIT_GAMES: usize = 10;
pub const HELD_OUT_GAMES: usize = 3;

/// Killed runs of the 3 that set a game's tier to `blocked`.
pub const KILLED_RUNS_TO_BLOCK: u32 = 2;

/// The phone every calibration run is on. More models come in a later phase.
pub const REFERENCE_PHONE: &str = "iPhone 14 Pro Max";

/// The calibration file the device thread fills in.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationFile {
    pub schema_version: u32,
    pub device: Device,
    #[serde(default, rename = "game")]
    pub games: Vec<GameRuns>,
}

/// The phone the runs were on, as recorded, and its baseline.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// The phone's model, as the person who ran it recorded it.
    pub phone: String,
    pub ios: String,
    /// `phys_footprint` of the harness page with no game frame, one per fresh run.
    pub baseline_bytes: Vec<u64>,
}

/// One game's fresh runs, with where its numbers came from.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameRuns {
    pub id: String,
    pub role: Role,
    /// The `dyno check` result of the build and its launch URL, relative to the calibration
    /// file.
    pub result: String,
    /// `phys_footprint` at idle, one per fresh run that was not killed.
    pub phone_bytes: Vec<u64>,
    #[serde(default)]
    pub killed_runs: u32,
    /// The dyno version that made the result, as a note for people.
    #[serde(default)]
    pub dyno_version: String,
    /// The result's `result_hash`, as a note for people.
    #[serde(default)]
    pub result_hash: String,
    /// The syslog line of each killed run.
    #[serde(default)]
    pub kill_lines: Vec<String>,
}

/// Whether a game sets the fit or tests it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Fit,
    HeldOut,
}

/// Where the parts of the model come from. Both take the JS heap from the browser run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipe {
    /// Decoded audio at 48 kHz, decoded textures (one copy) and the canvas floor, from the build.
    Static,
    /// Decoded audio at the context rate, resident textures and the canvas floor, from the run.
    Lab,
}

/// One game ready for the fit: its parts, its JS heap and its phone value.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub role: Role,
    /// The sum of the parts, in bytes.
    pub parts: f64,
    pub js_heap: f64,
    /// The median of the game's runs minus the median baseline. None when every run was killed.
    pub phone: Option<f64>,
    pub killed_runs: u32,
}

/// The published error report of one recipe.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub recipe: Recipe,
    pub k: f64,
    pub b: f64,
    pub games: Vec<GameError>,
    /// The worst `-signed_error` over games that underpredict; 0 when none does.
    pub worst_fit_underprediction: f64,
    pub worst_held_out_underprediction: f64,
    /// Whether the total may feed the provisional tier: all `HELD_OUT_GAMES` held-out games have
    /// a sound phone value, and their worst underprediction is under `GATE_UNDERPREDICTION`.
    pub feeds_the_tier: bool,
    /// The `playable` line less the headroom for the worst held-out underprediction, when the
    /// total feeds the tier.
    pub playable_line_bytes: Option<u64>,
    /// Where the runs differ from the method, such as a game with 2 runs.
    pub notes: Vec<String>,
}

/// One game's model, phone value and signed error.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GameError {
    pub id: String,
    pub role: Role,
    pub model_bytes: f64,
    /// None when every run was killed.
    pub phone_bytes: Option<f64>,
    /// (model - phone) / phone. Negative is an underprediction. None for a game left out.
    pub signed_error: Option<f64>,
    pub blocked_by_kills: bool,
    /// Why the game is left out of the fit and the gate. None for a game in them.
    pub left_out: Option<String>,
}

/// The self-checks a browser run must pass for its parts and heap to count.
const RUN_SELF_CHECKS: [MeasureId; 2] = [
    MeasureId::LabSelfCheckFrameDpr,
    MeasureId::LabSelfCheckFrameRequests,
];

/// The parts and JS heap of a result, by the recipe. An error names the measure that has no
/// value, or the self-check the browser run failed: a run at the wrong pixel ratio, or with no
/// requests, measured something other than the game.
pub fn parts(result: &CheckResult, recipe: Recipe) -> Result<(f64, f64), String> {
    let deterministic = &result.deterministic.measures;
    let run = &result.run.measures;
    for id in RUN_SELF_CHECKS {
        let check = run.iter().find(|measure| measure.id == id);
        if check.is_none_or(|check| check.status != MeasureStatus::Measured) {
            let reason = check.and_then(|check| check.reason.as_deref());
            return Err(format!(
                "the browser run failed its self-check {id}: {}",
                reason.unwrap_or("no reading")
            ));
        }
    }
    let ids: &[(MeasureId, &[Measure])] = match recipe {
        Recipe::Static => &[
            (MeasureId::StaticAudioDecoded48k, deterministic),
            (MeasureId::StaticTexturesDecoded, deterministic),
            (MeasureId::StaticRenderCanvas, deterministic),
        ],
        Recipe::Lab => &[
            (MeasureId::LabAudioDecoded, run),
            (MeasureId::LabTexturesResident, run),
            (MeasureId::LabCanvasBytes, run),
        ],
    };
    let mut sum = 0.0;
    for (id, measures) in ids {
        sum += bytes_of(measures, *id)?;
    }
    Ok((sum, bytes_of(run, MeasureId::LabJsHeap)?))
}

fn bytes_of(measures: &[Measure], id: MeasureId) -> Result<f64, String> {
    let measure = measures
        .iter()
        .find(|measure| measure.id == id)
        .ok_or_else(|| format!("{id} is not in the result"))?;
    let has_value = matches!(
        measure.status,
        MeasureStatus::Estimated | MeasureStatus::Measured
    );
    match (&measure.value, has_value) {
        (Some(MeasureValue::Integer(bytes)), true) => Ok(*bytes as f64),
        _ => Err(format!(
            "{id} has no value: {}",
            measure.reason.as_deref().unwrap_or("no reason")
        )),
    }
}

/// A game's row: its parts from its result, and its phone value against the device baseline. A
/// game whose every run was killed still gets a row, with no phone value.
pub fn row(game: &GameRuns, device: &Device, parts_and_heap: (f64, f64)) -> Result<Row, String> {
    let baseline = median(&device.baseline_bytes).ok_or("the device has no baseline runs")?;
    Ok(Row {
        id: game.id.clone(),
        role: game.role,
        parts: parts_and_heap.0,
        js_heap: parts_and_heap.1,
        phone: median(&game.phone_bytes).map(|runs| runs - baseline),
        killed_runs: game.killed_runs,
    })
}

impl Row {
    fn blocked_by_kills(&self) -> bool {
        self.killed_runs >= KILLED_RUNS_TO_BLOCK
    }

    /// Why the row cannot enter the fit or the gate, or None when its phone value is sound.
    fn left_out(&self) -> Option<String> {
        if self.blocked_by_kills() {
            return Some(format!(
                "blocked: {} of {RUNS_PER_GAME} runs were killed",
                self.killed_runs
            ));
        }
        match self.phone {
            None => Some("no run that was not killed".to_owned()),
            Some(phone) if phone <= 0.0 => {
                Some("its phone value is at or below the baseline".to_owned())
            }
            Some(_) => None,
        }
    }
}

/// Fits k and b on the sound fit games, then gives every game's signed error and the gate.
pub fn report(recipe: Recipe, rows: &[Row], file: &CalibrationFile) -> Report {
    let sound = |role| -> Vec<&Row> {
        rows.iter()
            .filter(|row| row.role == role && row.left_out().is_none())
            .collect()
    };
    let (k, b) = least_squares(&sound(Role::Fit));
    let games: Vec<GameError> = rows
        .iter()
        .map(|row| {
            let model = row.parts + k * row.js_heap + b;
            let left_out = row.left_out();
            GameError {
                id: row.id.clone(),
                role: row.role,
                model_bytes: model,
                phone_bytes: row.phone,
                signed_error: match (&left_out, row.phone) {
                    (None, Some(phone)) => Some((model - phone) / phone),
                    _ => None,
                },
                blocked_by_kills: row.blocked_by_kills(),
                left_out,
            }
        })
        .collect();
    let worst = |role| {
        games
            .iter()
            .filter(|game| game.role == role)
            .filter_map(|game| game.signed_error)
            .map(|error| -error)
            .fold(0.0_f64, f64::max)
    };
    let worst_fit_underprediction = worst(Role::Fit);
    let worst_held_out_underprediction = worst(Role::HeldOut);
    let enough_held_out = sound(Role::HeldOut).len() >= HELD_OUT_GAMES;
    let feeds_the_tier = enough_held_out && worst_held_out_underprediction < GATE_UNDERPREDICTION;
    let mut notes = method_notes(file, k);
    if !enough_held_out {
        notes.push(format!(
            "{} held-out games with a sound phone value, not {HELD_OUT_GAMES}: the total does not \
             feed the tier",
            sound(Role::HeldOut).len()
        ));
    }
    Report {
        recipe,
        k,
        b,
        games,
        worst_fit_underprediction,
        worst_held_out_underprediction,
        feeds_the_tier,
        playable_line_bytes: feeds_the_tier.then_some(
            (PLAYABLE_LINE_BYTES as f64 * (1.0 - worst_held_out_underprediction)) as u64,
        ),
        notes,
    }
}

/// k and b that minimise the squared error of `phone - parts = k x heap + b` over the rows. With
/// every heap the same, k is 0 and b is the mean. Rows with no phone value are skipped.
fn least_squares(rows: &[&Row]) -> (f64, f64) {
    let points: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|row| Some((row.js_heap, row.phone? - row.parts)))
        .collect();
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let count = points.len() as f64;
    let mean_heap = points.iter().map(|(heap, _)| heap).sum::<f64>() / count;
    let mean_rest = points.iter().map(|(_, rest)| rest).sum::<f64>() / count;
    let covariance: f64 = points
        .iter()
        .map(|(heap, rest)| (heap - mean_heap) * (rest - mean_rest))
        .sum();
    let variance: f64 = points
        .iter()
        .map(|(heap, _)| (heap - mean_heap).powi(2))
        .sum();
    let k = if variance > 0.0 {
        covariance / variance
    } else {
        0.0
    };
    (k, mean_rest - k * mean_heap)
}

/// Where the runs differ from the calibration method.
fn method_notes(file: &CalibrationFile, k: f64) -> Vec<String> {
    let mut notes = Vec::new();
    if file.device.phone != REFERENCE_PHONE {
        notes.push(format!(
            "the runs are on \"{}\", not the reference phone, the {REFERENCE_PHONE}",
            file.device.phone
        ));
    }
    if file.device.ios.trim().is_empty() {
        notes.push("no iOS version recorded".to_owned());
    }
    let count = |role| file.games.iter().filter(|game| game.role == role).count();
    if count(Role::Fit) != FIT_GAMES {
        notes.push(format!("{} fit games, not {FIT_GAMES}", count(Role::Fit)));
    }
    if count(Role::HeldOut) != HELD_OUT_GAMES {
        notes.push(format!(
            "{} held-out games, not {HELD_OUT_GAMES}",
            count(Role::HeldOut)
        ));
    }
    if file.device.baseline_bytes.len() != RUNS_PER_GAME {
        notes.push(format!(
            "{} baseline runs, not {RUNS_PER_GAME}",
            file.device.baseline_bytes.len()
        ));
    }
    for game in &file.games {
        let runs = game.phone_bytes.len() + game.killed_runs as usize;
        if runs != RUNS_PER_GAME {
            notes.push(format!("{}: {runs} runs, not {RUNS_PER_GAME}", game.id));
        }
        // A killed run had the highest footprint, so the runs left give a value biased low.
        if game.killed_runs > 0 && game.killed_runs < KILLED_RUNS_TO_BLOCK {
            notes.push(format!(
                "{}: {} killed run, so its phone value is the median of the {} left, biased low",
                game.id,
                game.killed_runs,
                game.phone_bytes.len()
            ));
        }
    }
    if k < 0.0 {
        notes.push(format!(
            "k is negative ({k:.3}): a larger heap lowers the model"
        ));
    }
    notes
}

/// The median, the mean of the middle two for an even count. None for no values.
fn median(values: &[u64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        count if count % 2 == 1 => Some(sorted[middle] as f64),
        _ => Some((sorted[middle - 1] as f64 + sorted[middle] as f64) / 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: f64 = 1_048_576.0;

    fn file(games: Vec<GameRuns>) -> CalibrationFile {
        CalibrationFile {
            schema_version: 0,
            device: device(),
            games,
        }
    }

    fn device() -> Device {
        Device {
            phone: REFERENCE_PHONE.to_owned(),
            ios: "18.6".to_owned(),
            baseline_bytes: vec![100, 120, 110].into_iter().map(mb).collect(),
        }
    }

    fn mb(megabytes: u64) -> u64 {
        megabytes * 1_048_576
    }

    fn game(id: &str, role: Role, runs: &[u64]) -> GameRuns {
        GameRuns {
            id: id.to_owned(),
            role,
            result: format!("{id}.json"),
            phone_bytes: runs.iter().copied().map(mb).collect(),
            killed_runs: 0,
            dyno_version: String::new(),
            result_hash: String::new(),
            kill_lines: Vec::new(),
        }
    }

    fn fit_row(id: &str, role: Role, parts_mb: f64, heap_mb: f64, phone_mb: f64) -> Row {
        Row {
            id: id.to_owned(),
            role,
            parts: parts_mb * MB,
            js_heap: heap_mb * MB,
            phone: Some(phone_mb * MB),
            killed_runs: 0,
        }
    }

    /// Three held-out games with the same model and phone values, so the gate has its count.
    fn held_out(parts_mb: f64, phone_mb: f64) -> Vec<Row> {
        (0..HELD_OUT_GAMES)
            .map(|index| {
                fit_row(
                    &format!("held{index}"),
                    Role::HeldOut,
                    parts_mb,
                    10.0,
                    phone_mb,
                )
            })
            .collect()
    }

    fn base_rows() -> Vec<Row> {
        vec![
            fit_row("a", Role::Fit, 100.0, 10.0, 100.0),
            fit_row("b", Role::Fit, 100.0, 10.0, 100.0),
        ]
    }

    #[test]
    fn the_phone_value_is_the_median_run_less_the_median_baseline() {
        let runs = game("a", Role::Fit, &[400, 300, 350]);
        let row = row(&runs, &device(), (0.0, 0.0)).unwrap();
        assert_eq!(row.phone, Some(240.0 * MB));
    }

    #[test]
    fn an_even_count_takes_the_mean_of_the_middle_two() {
        assert_eq!(median(&[1, 4, 2, 3]), Some(2.5));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn two_killed_runs_block_the_game_and_leave_it_out() {
        let mut runs = game("blocked", Role::HeldOut, &[400]);
        runs.killed_runs = 2;
        let blocked = row(&runs, &device(), (0.0, 0.0)).unwrap();
        let mut rows = base_rows();
        rows.push(blocked);
        let report = report(Recipe::Lab, &rows, &file(Vec::new()));
        let game = report
            .games
            .iter()
            .find(|game| game.id == "blocked")
            .unwrap();
        assert!(game.blocked_by_kills);
        assert_eq!(game.signed_error, None);
        assert!(game.left_out.as_deref().unwrap().starts_with("blocked"));
    }

    #[test]
    fn a_game_killed_in_every_run_is_reported_not_an_error() {
        let mut runs = game("killed", Role::Fit, &[]);
        runs.killed_runs = 3;
        let killed = row(&runs, &device(), (50.0 * MB, MB)).unwrap();
        assert_eq!(killed.phone, None);
        let mut rows = base_rows();
        rows.push(killed);
        let report = report(Recipe::Lab, &rows, &file(Vec::new()));
        let game = report
            .games
            .iter()
            .find(|game| game.id == "killed")
            .unwrap();
        assert!(game.blocked_by_kills);
        assert_eq!(game.phone_bytes, None);
    }

    #[test]
    fn a_phone_value_at_or_below_the_baseline_is_left_out() {
        let mut rows = base_rows();
        rows.push(fit_row("under", Role::Fit, 10.0, 10.0, -5.0));
        rows.push(fit_row("zero", Role::Fit, 10.0, 10.0, 0.0));
        let report = report(Recipe::Static, &rows, &file(Vec::new()));
        for id in ["under", "zero"] {
            let game = report.games.iter().find(|game| game.id == id).unwrap();
            assert_eq!(game.signed_error, None, "{id}");
            assert!(game.left_out.is_some(), "{id}");
        }
        assert_eq!(report.worst_fit_underprediction, 0.0);
    }

    #[test]
    fn the_fit_recovers_k_and_b_from_exact_rows() {
        // phone = parts + 2 x heap + 50 MB.
        let rows = vec![
            fit_row("a", Role::Fit, 100.0, 10.0, 170.0),
            fit_row("b", Role::Fit, 200.0, 20.0, 290.0),
            fit_row("c", Role::Fit, 50.0, 40.0, 180.0),
        ];
        let report = report(Recipe::Lab, &rows, &file(Vec::new()));
        assert!((report.k - 2.0).abs() < 1e-9, "{}", report.k);
        assert!((report.b - 50.0 * MB).abs() < 1.0, "{}", report.b);
        assert!(
            report
                .games
                .iter()
                .all(|game| game.signed_error.unwrap().abs() < 1e-9)
        );
    }

    #[test]
    fn the_signed_error_is_negative_for_an_underprediction() {
        let mut rows = base_rows();
        rows.extend(held_out(100.0, 125.0));
        let report = report(Recipe::Static, &rows, &file(Vec::new()));
        let held = report.games.iter().find(|game| game.id == "held0").unwrap();
        assert!((held.signed_error.unwrap() + 0.2).abs() < 1e-9);
        assert!((report.worst_held_out_underprediction - 0.2).abs() < 1e-9);
        assert!(!report.feeds_the_tier, "20% is not under 20%");
        assert_eq!(report.playable_line_bytes, None);
    }

    #[test]
    fn a_held_out_underprediction_under_20_percent_feeds_the_tier_with_headroom() {
        let mut rows = base_rows();
        rows.extend(held_out(85.0, 100.0));
        let report = report(Recipe::Static, &rows, &file(Vec::new()));
        assert!((report.worst_held_out_underprediction - 0.15).abs() < 1e-9);
        assert!(report.feeds_the_tier);
        let line = report.playable_line_bytes.unwrap() as f64;
        assert!((line - 425.0 * MB).abs() < 2.0, "{line}");
    }

    #[test]
    fn fewer_than_3_sound_held_out_games_never_feed_the_tier() {
        let mut rows = base_rows();
        rows.push(fit_row("held", Role::HeldOut, 100.0, 10.0, 100.0));
        let one = report(Recipe::Static, &rows, &file(Vec::new()));
        assert!(!one.feeds_the_tier);
        assert!(
            one.notes
                .iter()
                .any(|note| note.starts_with("1 held-out games with a sound"))
        );
        let mut blocked_third = base_rows();
        blocked_third.extend(held_out(100.0, 100.0));
        blocked_third[4].killed_runs = 2;
        assert!(!report(Recipe::Static, &blocked_third, &file(Vec::new())).feeds_the_tier);
    }

    #[test]
    fn the_notes_name_where_the_runs_differ_from_the_method() {
        let mut one_kill = game("one-kill", Role::Fit, &[300, 310]);
        one_kill.killed_runs = 1;
        let calibration = file(vec![game("short", Role::Fit, &[300, 310]), one_kill]);
        let notes = method_notes(&calibration, -0.5);
        assert!(notes.contains(&"2 fit games, not 10".to_owned()));
        assert!(notes.contains(&"0 held-out games, not 3".to_owned()));
        assert!(notes.contains(&"short: 2 runs, not 3".to_owned()));
        assert!(
            notes
                .iter()
                .any(|note| note.starts_with("one-kill: 1 killed run"))
        );
        assert!(notes.iter().any(|note| note.starts_with("k is negative")));
    }

    #[test]
    fn the_template_parses_and_holds_no_runs() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/calibration/template.toml");
        let text = std::fs::read_to_string(path).unwrap();
        let template: CalibrationFile = toml::from_str(&text).unwrap();
        assert!(template.device.baseline_bytes.is_empty());
        assert!(
            template
                .games
                .iter()
                .all(|game| game.phone_bytes.is_empty())
        );
        assert_eq!(template.device.phone, REFERENCE_PHONE);
    }

    #[test]
    fn runs_on_the_reference_phone_with_an_ios_version_need_no_device_note() {
        let notes = method_notes(&file(Vec::new()), 1.0);
        assert!(!notes.iter().any(|note| note.contains("phone")));
        assert!(!notes.iter().any(|note| note.contains("iOS")));
    }

    #[test]
    fn the_notes_name_another_phone_and_a_missing_ios_version() {
        let mut calibration = file(Vec::new());
        calibration.device.phone = "iPhone 13 Pro Max".to_owned();
        calibration.device.ios = String::new();
        let notes = method_notes(&calibration, 1.0);
        assert!(notes.contains(
            &"the runs are on \"iPhone 13 Pro Max\", not the reference phone, the iPhone 14 Pro Max"
                .to_owned()
        ));
        assert!(notes.contains(&"no iOS version recorded".to_owned()));
    }
}
