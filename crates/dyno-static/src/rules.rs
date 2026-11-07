//! Static rules: findings that explain the summaries and suggest fixes with the bytes they save.
//!
//! A finding's verdict counts toward the overall status (dyno-score `status_of_finding`). A rule
//! reads the summaries and the budget file's limits. A limit the file does not set takes its proposal default (design section 9).

use std::borrow::Borrow;
use std::cmp::Reverse;

use dyno_schema::SCHEMA_VERSION;
use dyno_schema::budget::{
    AudioBudget, BYTES_PER_MB, Budget, BudgetLimit, FilesBudget, RenderBudget, Severity,
    ShareLimit, SizeLimit, TextureBudget,
};
use dyno_schema::ids::{BudgetId, RuleId};
use dyno_schema::measure::{MeasureStatus, NOT_BUILT_YET};
use dyno_schema::path::RelPath;
use dyno_schema::rule::{Finding, Fix, FixAction, incomplete_findings};
use dyno_schema::status::Verdict;

use crate::NEEDS_A_BUILD;
use crate::audio::StereoClass;
use crate::duplicates::extra_copies;
use crate::model::{
    AudioSummary, BundleSummary, DuplicateGroup, RESOLUTION_CAP_MILLI, RenderSite,
    SpriteMapSummary, StereoSound, Summaries, TextureCost, TextureSummary, canvas_floor,
    resolution_text,
};

/// One rule's finding, with what the result has no field for: the rule's value, the files it
/// names and the fix text (design section 9.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleReport {
    pub finding: Finding,
    /// None when the rule stopped before it had a value.
    pub value: Option<RuleValue>,
    /// The fixes' paths, largest saving first, then by path. The render rule names the worst
    /// site's file, and `files.duplicate` names each group by its first path.
    pub items: Vec<RelPath>,
    /// The fix text for a text report. None when there is no fix.
    pub text: Option<String>,
}

/// A rule's value, in the unit of what it checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleValue {
    Bytes(u64),
    BasisPoints(u64),
    Count(u64),
}

/// A verdict, with the reason when it is incomplete.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Pass,
    Above(Level),
    Incomplete(String),
}

/// The level of a limit. Fail is the worse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Warn,
    Fail,
}

/// A warn and a fail limit in one unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Thresholds {
    warn_above: Option<u64>,
    fail_above: Option<u64>,
}

/// The budget file's limits, with the default for each key the file does not set.
struct Limits<'a> {
    file: Option<&'a Budget>,
}

/// The static rules, Phase 1 and 2.
pub const RULES: &[RuleId] = &[
    RuleId::AudioPcmOverBudget,
    RuleId::AudioStereo,
    RuleId::AudioSilenceShare,
    RuleId::RenderAntialiasOrResolution,
    RuleId::TexturesOverBudget,
    RuleId::FilesDuplicate,
    RuleId::FilesUnreferenced,
];

/// The limit for each budget key a check's budget file does not set, or for every key when there
/// is no file. They are proposals until device runs set them (design section 9.1), and
/// `examples/budget.toml` holds the same values.
pub const DEFAULT_BUDGET: Budget = Budget {
    schema_version: SCHEMA_VERSION,
    name: None,
    audio: AudioBudget {
        decoded_mb: Some(SizeLimit {
            warn: Some(64),
            fail: Some(128),
        }),
        stereo_saving_mb: Some(SizeLimit {
            warn: Some(4),
            fail: None,
        }),
        silence_share_bp: Some(ShareLimit {
            warn: Some(1_000),
            fail: None,
        }),
        silence_saving_mb: Some(SizeLimit {
            warn: Some(1),
            fail: None,
        }),
    },
    textures: TextureBudget {
        decoded_mb: Some(SizeLimit {
            warn: Some(160),
            fail: Some(256),
        }),
    },
    render: RenderBudget {
        antialias: Some(Severity::Warn),
        resolution_uncapped: Some(Severity::Warn),
    },
    files: FilesBudget {
        duplicate: Some(Severity::Warn),
    },
};

/// Why the render rule is incomplete when the bundle sets no renderer options.
pub const NO_RENDER_CONFIG: &str = "no render config found";

/// Why the render rule is incomplete when no render site's Pixi can start.
pub const NO_CANVAS: &str = "no render config can start a renderer";

/// What the render rule says about antialias: its saving needs a device run.
const ANTIALIAS_ADVICE: &str = "Set `antialias: false`. A device run will measure what it saves, so this fix has no number yet.";

/// How many of the largest textures `textures.over_budget` names.
const TOP_TEXTURES: usize = 5;

/// How many paths a text or a reason names before "and N more".
const NAMED_PATHS: usize = 5;

const BYTES_PER_KB: u64 = 1_024;

const BASIS_POINTS_PER_PERCENT: u64 = 100;

/// The levels in the order a value is checked against them.
const WORST_LEVEL_FIRST: [Level; 2] = [Level::Fail, Level::Warn];

/// One finding per static rule.
pub fn check(summaries: &Summaries, budget: Option<&Budget>) -> Vec<Finding> {
    reports(summaries, budget)
        .into_iter()
        .map(|report| report.finding)
        .collect()
}

/// One report per static rule, in the order of `RULES`.
///
/// A rule that would pass while a measure it reads has no value is incomplete with that measure's
/// reason: its family left the summary empty, which reads as a build with nothing to flag. The
/// render rule is the exception: it reads each site itself, and its measures have no value on
/// purpose for a game that draws with HTML or a 2D canvas, where the rule passes.
pub fn reports(summaries: &Summaries, budget: Option<&Budget>) -> Vec<RuleReport> {
    let limits = Limits { file: budget };
    RULES
        .iter()
        .map(|&rule| {
            let report = report(rule, summaries, &limits);
            let reads_its_own_gaps = rule == RuleId::RenderAntialiasOrResolution;
            match measure_without_value(rule, summaries) {
                Some(reason) if report.finding.verdict == Verdict::Pass && !reads_its_own_gaps => {
                    RuleReport::incomplete(rule, &reason)
                }
                _ => report,
            }
        })
        .collect()
}

/// One incomplete finding per static rule, for a check with no build.
pub fn without_build() -> Vec<Finding> {
    incomplete_findings(RULES, NEEDS_A_BUILD)
}

impl RuleReport {
    /// The sum of the fixes' savings: download bytes for `files.duplicate`, decoded bytes for
    /// the other rules.
    pub fn saved_bytes(&self) -> u64 {
        self.finding.fixes.iter().map(|fix| fix.saves_bytes).sum()
    }

    /// The decoded bytes that removing duplicate copies also saves. 0 for the other rules.
    pub fn saved_decoded_bytes(&self) -> u64 {
        self.finding
            .fixes
            .iter()
            .filter_map(|fix| fix.saves_decoded_bytes)
            .sum()
    }

    /// A report with a value and fixes. The items are the fixes' paths, and the text goes only
    /// with fixes.
    fn new(
        rule: RuleId,
        outcome: Outcome,
        value: RuleValue,
        fixes: Vec<Fix>,
        text: String,
    ) -> RuleReport {
        let spec = rule.spec();
        let (verdict, reason) = outcome.into_verdict();
        RuleReport {
            value: Some(value),
            items: paths_by_saving(&fixes),
            text: (!fixes.is_empty()).then_some(text),
            finding: Finding {
                rule,
                verdict,
                phase: spec.phase,
                measures: spec.measures.to_vec(),
                fixes,
                reason,
            },
        }
    }

    fn incomplete(rule: RuleId, reason: &str) -> RuleReport {
        RuleReport {
            finding: Finding::incomplete(rule, reason),
            value: None,
            items: Vec::new(),
            text: None,
        }
    }
}

fn report(rule: RuleId, summaries: &Summaries, limits: &Limits) -> RuleReport {
    match rule {
        RuleId::AudioPcmOverBudget => pcm_over_budget(&summaries.audio, limits),
        RuleId::AudioStereo => stereo(&summaries.audio, limits),
        RuleId::AudioSilenceShare => silence_share(&summaries.audio, &summaries.bundle, limits),
        RuleId::RenderAntialiasOrResolution => antialias_or_resolution(&summaries.bundle, limits),
        RuleId::TexturesOverBudget => textures_over_budget(&summaries.textures, limits),
        RuleId::FilesDuplicate => duplicate_files(&summaries.duplicates, limits),
        RuleId::FilesUnreferenced => RuleReport::incomplete(rule, NOT_BUILT_YET),
        other => unreachable!("{other} is not a static rule"),
    }
}

/// "<measure id>: <reason>" for the first measure the rule reads that has no value.
fn measure_without_value(rule: RuleId, summaries: &Summaries) -> Option<String> {
    let reads = rule.spec().measures;
    summaries
        .measures_without_value
        .iter()
        .find(|measure| reads.contains(&measure.id))
        .map(|measure| format!("{}: {}", measure.id, measure.reason))
}

/// Decoded audio at 48 kHz against `audio.decoded_mb`. An AudioContext at 24 kHz halves it.
fn pcm_over_budget(audio: &AudioSummary, limits: &Limits) -> RuleReport {
    let value = audio.decoded_bytes_48k;
    let saving = value.saturating_sub(audio.decoded_bytes_24k);
    let thresholds = limits.thresholds(BudgetId::AudioDecodedMb);
    let outcome = budget_outcome(thresholds, value, &audio.not_covered);
    let fixes = if saving > 0 {
        vec![fix(FixAction::ResampleTo24k, None, saving)]
    } else {
        Vec::new()
    };
    let text = format!(
        "Decoded audio is {} MB at 48 kHz. Run the AudioContext at 24 kHz to save {} MB, or load \
         long music when it is needed.",
        mb(value),
        mb(saving)
    );
    RuleReport::new(
        RuleId::AudioPcmOverBudget,
        outcome,
        RuleValue::Bytes(value),
        fixes,
        text,
    )
}

/// What a mono export of each stereo sound saves, against `audio.stereo_saving_mb`. A sound
/// counts once, at its group's representative. A sound whose decode shows that mono would cancel
/// it out of phase keeps its stereo and gets no fix. The text asks for a listen first when mono
/// would change the sound. A sound that did not decode is judged by its channel count alone.
fn stereo(audio: &AudioSummary, limits: &Limits) -> RuleReport {
    let (cancels, mono_candidates): (Vec<&StereoSound>, Vec<&StereoSound>) = audio
        .stereo_groups
        .iter()
        .partition(|sound| sound.class == Some(StereoClass::Cancels));
    // A sound too short to halve has nothing to save.
    let halvable: Vec<&StereoSound> = mono_candidates
        .into_iter()
        .filter(|sound| sound.decoded_bytes_48k / 2 > 0)
        .collect();
    let fixes: Vec<Fix> = halvable
        .iter()
        .map(|sound| {
            let mono_saving = sound.decoded_bytes_48k / 2;
            fix(FixAction::DownmixToMono, Some(&sound.path), mono_saving)
        })
        .collect();
    let value = fixes.iter().map(|fix| fix.saves_bytes).sum();
    let thresholds = limits.thresholds(BudgetId::AudioStereoSavingMb);
    let outcome = budget_outcome(thresholds, value, &audio.not_covered);
    let decoded = halvable.iter().map(|sound| sound.decoded_bytes_48k).sum();
    let sounds = count(fixes.len(), "stereo sound");
    let decode = if fixes.len() == 1 {
        "decodes"
    } else {
        "decode"
    };
    let them = it_or_them(fixes.len());
    let mut text = format!(
        "{sounds} {decode} to {} MB. Export {them} as mono to save {} MB.",
        mb(decoded),
        mb(value)
    );
    let changes = halvable
        .iter()
        .filter(|sound| sound.class == Some(StereoClass::ChangesTheSound))
        .count();
    if changes > 0 {
        let which = match (changes, fixes.len()) {
            (1, 1) => "its sound".to_owned(),
            (changes, all) if changes == all => format!("the sound of all {changes}"),
            (changes, _) => format!("the sound of {changes} of them"),
        };
        text.push_str(&format!(" Mono would change {which}, so listen first."));
    }
    if !cancels.is_empty() {
        let as_it_is = if cancels.len() == 1 {
            "it is"
        } else {
            "they are"
        };
        text.push_str(&format!(
            " Keep {} as {as_it_is}: mono would cancel {} out of phase.",
            count(cancels.len(), "other stereo sound"),
            it_or_them(cancels.len())
        ));
    }
    RuleReport::new(
        RuleId::AudioStereo,
        outcome,
        RuleValue::Bytes(value),
        fixes,
        text,
    )
}

/// The time no sprite covers in each sprite map. A map reaches a level only above that level's
/// limit in both silence keys, so a short file with a long gap stays quiet.
fn silence_share(audio: &AudioSummary, bundle: &BundleSummary, limits: &Limits) -> RuleReport {
    let share = limits.thresholds(BudgetId::AudioSilenceShareBp);
    let saving = limits.thresholds(BudgetId::AudioSilenceSavingMb);
    let estimated: Vec<&SpriteMapSummary> = audio
        .sprite_maps
        .iter()
        .filter(|map| map.status == MeasureStatus::Estimated)
        .collect();
    let value = estimated
        .iter()
        .map(|map| u64::from(map.silence_bp))
        .max()
        .unwrap_or(0);
    let maps_above = |level| -> Vec<&SpriteMapSummary> {
        let is_above = |map: &&SpriteMapSummary| {
            share.is_above(level, u64::from(map.silence_bp))
                && saving.is_above(level, map.silence_bytes_48k)
        };
        estimated.iter().copied().filter(is_above).collect()
    };
    let worst = WORST_LEVEL_FIRST
        .into_iter()
        .map(|level| (level, maps_above(level)))
        .find(|(_, maps)| !maps.is_empty());
    let (outcome, maps) = match worst {
        Some((level, maps)) => (Outcome::Above(level), maps),
        None => {
            let outcome =
                unchecked_sprite_maps(audio, bundle).map_or(Outcome::Pass, Outcome::Incomplete);
            (outcome, Vec::new())
        }
    };
    let fixes = maps
        .iter()
        .map(|map| {
            let silence = map.silence_bytes_48k;
            fix(FixAction::TrimSpriteSilence, Some(&map.source), silence)
        })
        .collect();
    let text = maps
        .iter()
        .map(|map| silence_text(map))
        .collect::<Vec<_>>()
        .join(" ");
    RuleReport::new(
        RuleId::AudioSilenceShare,
        outcome,
        RuleValue::BasisPoints(value),
        fixes,
        text,
    )
}

/// What the render sites prove: antialias on where Pixi may draw on the GPU, and a resolution
/// above `RESOLUTION_CAP_MILLI`. Each gets its own budget key's severity. A proven warning wins
/// over a site that sets its options at run time. Only a fixed resolution gets a fix with a
/// saving; turning antialias off has no fix until a device run measures what it saves.
fn antialias_or_resolution(bundle: &BundleSummary, limits: &Limits) -> RuleReport {
    let rule = RuleId::RenderAntialiasOrResolution;
    if bundle.render_sites.is_empty() && bundle.draws_with_html {
        // No renderer to configure. Its measures are unsupported, so the run checks the page.
        return RuleReport::new(
            rule,
            Outcome::Pass,
            RuleValue::Bytes(0),
            Vec::new(),
            String::new(),
        );
    }
    if bundle.render_sites.is_empty() {
        return RuleReport::incomplete(rule, NO_RENDER_CONFIG);
    }
    let drawing: Vec<&RenderSite> = bundle
        .render_sites
        .iter()
        .filter(|site| site.canvas.is_some())
        .collect();
    let Some(worst) = largest_floor(&drawing) else {
        return RuleReport::incomplete(rule, NO_CANVAS);
    };
    let on_the_gpu = |site: &&&RenderSite| !site.draws_in_2d;
    let antialias_on = drawing
        .iter()
        .filter(on_the_gpu)
        .filter(|site| site.antialias == Some(true))
        .map(|site| site.file.clone())
        .collect::<Vec<_>>();
    let uncapped: Vec<&RenderSite> = drawing
        .iter()
        .copied()
        .filter(|site| site.resolution.is_above(RESOLUTION_CAP_MILLI) == Some(true))
        .collect();
    let mut levels = Vec::new();
    if !antialias_on.is_empty() {
        levels.push(limits.severity(BudgetId::RenderAntialias));
    }
    if !uncapped.is_empty() {
        levels.push(limits.severity(BudgetId::RenderResolutionUncapped));
    }
    let fixes: Vec<Fix> = uncapped
        .iter()
        .filter_map(|site| cap_resolution(site))
        .collect();
    let outcome = match levels.into_iter().filter_map(level_of).max() {
        Some(level) => Outcome::Above(level),
        None => match unknown_render_options(&drawing) {
            Some(reason) => Outcome::Incomplete(reason),
            // A script that did not parse may hold a worse render site.
            None => unparsed_scripts(bundle, "a render config")
                .map_or(Outcome::Pass, Outcome::Incomplete),
        },
    };
    let mut sentences = Vec::new();
    if !antialias_on.is_empty() {
        sentences.push(format!(
            "{} {} antialias on. {ANTIALIAS_ADVICE}",
            path_list(&antialias_on),
            if antialias_on.len() == 1 {
                "turns"
            } else {
                "turn"
            }
        ));
    }
    for site in &uncapped {
        sentences.push(resolution_sentence(site));
    }
    let mut items = paths_by_saving(&fixes);
    for file in antialias_on
        .iter()
        .chain(uncapped.iter().map(|site| &site.file))
    {
        if !items.contains(file) {
            items.push(file.clone());
        }
    }
    if items.is_empty() {
        items.push(worst.file.clone());
    }
    let floor = worst.canvas.map_or(0, |canvas| canvas.bytes);
    let mut report = RuleReport::new(rule, outcome, RuleValue::Bytes(floor), fixes, String::new());
    report.items = items;
    report.text = (!sentences.is_empty()).then(|| sentences.join(" "));
    report
}

/// The fix for a site whose resolution is fixed above the cap: the colour buffer pixels the cap
/// removes.
fn cap_resolution(site: &RenderSite) -> Option<Fix> {
    let resolution_milli = site.resolution.fixed()?;
    let now = canvas_floor(site.canvas_css, resolution_milli).bytes;
    let capped = canvas_floor(site.canvas_css, RESOLUTION_CAP_MILLI).bytes;
    Some(fix(
        FixAction::CapResolution,
        None,
        now.saturating_sub(capped),
    ))
}

/// What the rule says about a site whose resolution is above the cap.
fn resolution_sentence(site: &RenderSite) -> String {
    let cap = resolution_text(RESOLUTION_CAP_MILLI);
    let advice = format!("Cap `resolution` at {cap} (`Math.min(devicePixelRatio, {cap})`)");
    match (site.resolution.fixed(), cap_resolution(site)) {
        (Some(milli), Some(fix)) => format!(
            "{} sets resolution {}. {advice} to save {} MB.",
            site.file,
            resolution_text(milli),
            mb(fix.saves_bytes)
        ),
        _ => format!("{} sets a resolution above {cap}. {advice}.", site.file),
    }
}

/// Why the rule cannot pass: the first site that sets antialias (where it matters) or the
/// resolution at run time. None when every site's options are known.
fn unknown_render_options(drawing: &[&RenderSite]) -> Option<String> {
    drawing.iter().find_map(|site| {
        let antialias_unknown = !site.draws_in_2d && site.antialias.is_none();
        let resolution_unknown = site.resolution.is_above(RESOLUTION_CAP_MILLI).is_none();
        let what = match (antialias_unknown, resolution_unknown) {
            (true, true) => "antialias and the resolution",
            (true, false) => "antialias",
            (false, true) => "the resolution",
            (false, false) => return None,
        };
        Some(format!("{} sets {what} at run time", site.file))
    })
}

/// Decoded textures, one copy, against `textures.decoded_mb`. Names the largest files that ASTC
/// would shrink, and what it would save on each.
fn textures_over_budget(textures: &TextureSummary, limits: &Limits) -> RuleReport {
    let value = textures.low_bytes;
    let thresholds = limits.thresholds(BudgetId::TexturesDecodedMb);
    let outcome = budget_outcome(thresholds, value, &textures.not_covered);
    let mut largest: Vec<&TextureCost> = textures.files.iter().collect();
    // The files are sorted by path and the sort is stable, so ties stay in path order.
    largest.sort_by_key(|file| Reverse(file.gpu_bytes));
    largest.truncate(TOP_TEXTURES);
    // A file already in an ASTC-sized format has nothing to shrink.
    let fixes: Vec<Fix> = largest
        .iter()
        .filter(|file| file.gpu_bytes > file.astc_bytes)
        .map(|file| {
            let saving = file.gpu_bytes - file.astc_bytes;
            fix(FixAction::ShrinkTextures, Some(&file.path), saving)
        })
        .collect();
    let names: Vec<&RelPath> = fixes.iter().filter_map(|fix| fix.path.as_ref()).collect();
    let largest_files = match names.len() {
        1 => "The largest".to_owned(),
        amount => format!("The {amount} largest"),
    };
    let text = format!(
        "Textures decode to {} to {} MB. {largest_files}: {}. Ship {} as KTX2 (Basis) to save \
         about {} MB.",
        mb(textures.low_bytes),
        mb(textures.high_bytes),
        path_list(&names),
        it_or_them(names.len()),
        mb(fixes.iter().map(|fix| fix.saves_bytes).sum())
    );
    RuleReport::new(
        RuleId::TexturesOverBudget,
        outcome,
        RuleValue::Bytes(value),
        fixes,
        text,
    )
}

/// Files that repeat another byte for byte. Any group gets the `files.duplicate` severity.
fn duplicate_files(groups: &[DuplicateGroup], limits: &Limits) -> RuleReport {
    let outcome = if groups.is_empty() {
        Outcome::Pass
    } else {
        Outcome::from_level(level_of(limits.severity(BudgetId::FilesDuplicate)))
    };
    let fixes: Vec<Fix> = groups.iter().flat_map(copy_removals).collect();
    let fix_paths = |action| -> Vec<&RelPath> {
        fixes
            .iter()
            .filter(|fix| fix.action == action)
            .filter_map(|fix| fix.path.as_ref())
            .collect()
    };
    let deletable = fix_paths(FixAction::RemoveDuplicate);
    let unclear = fix_paths(FixAction::KeepOneCopy);
    let mut sentences = Vec::new();
    if !deletable.is_empty() {
        let are_copies = match deletable.len() {
            1 => "1 file is a copy of a file the game names",
            _ => "files are copies of files the game names",
        };
        let count = if deletable.len() == 1 {
            String::new()
        } else {
            format!("{} ", deletable.len())
        };
        sentences.push(format!(
            "{count}{are_copies}: {}. Delete {}.",
            path_list(&deletable),
            it_or_them(deletable.len())
        ));
    }
    if !unclear.is_empty() {
        sentences.push(format!(
            "The build does not show which copy the game loads of {}. Keep that copy and delete \
             the others.",
            path_list(&unclear)
        ));
    }
    let download: u64 = fixes.iter().map(|fix| fix.saves_bytes).sum();
    let decoded: u64 = fixes.iter().filter_map(|fix| fix.saves_decoded_bytes).sum();
    sentences.push(format!(
        "This saves {} KB to download and up to {} MB decoded.",
        kb(download),
        mb(decoded)
    ));
    let text = sentences.join(" ");
    let mut by_waste: Vec<&DuplicateGroup> = groups.iter().collect();
    // The groups are sorted by first path and the sort is stable, so ties stay in path order.
    by_waste.sort_by_key(|group| Reverse(group.wasted_bytes));
    RuleReport {
        items: by_waste
            .iter()
            .filter_map(|group| group.paths.first().cloned())
            .collect(),
        ..RuleReport::new(
            RuleId::FilesDuplicate,
            outcome,
            RuleValue::Count(groups.len() as u64),
            fixes,
            text,
        )
    }
}

impl Outcome {
    fn from_level(level: Option<Level>) -> Outcome {
        level.map_or(Outcome::Pass, Outcome::Above)
    }

    fn into_verdict(self) -> (Verdict, Option<String>) {
        match self {
            Outcome::Pass => (Verdict::Pass, None),
            Outcome::Above(Level::Warn) => (Verdict::Warn, None),
            Outcome::Above(Level::Fail) => (Verdict::Fail, None),
            Outcome::Incomplete(reason) => (Verdict::Incomplete, Some(reason)),
        }
    }
}

impl Thresholds {
    /// The worse level whose limit the value is above, if any.
    fn level(self, value: u64) -> Option<Level> {
        WORST_LEVEL_FIRST
            .into_iter()
            .find(|&level| self.is_above(level, value))
    }

    /// Whether the value is above this level's limit. A level with no limit is never reached.
    fn is_above(self, level: Level, value: u64) -> bool {
        let limit = match level {
            Level::Warn => self.warn_above,
            Level::Fail => self.fail_above,
        };
        limit.is_some_and(|limit| value > limit)
    }
}

impl Limits<'_> {
    fn thresholds(&self, budget: BudgetId) -> Thresholds {
        match self.limit(budget) {
            BudgetLimit::Bytes {
                warn_above,
                fail_above,
            }
            | BudgetLimit::BasisPoints {
                warn_above,
                fail_above,
            } => Thresholds {
                warn_above,
                fail_above,
            },
            BudgetLimit::Flag { .. } => unreachable!("{budget} is a flag, not a limit"),
        }
    }

    fn severity(&self, budget: BudgetId) -> Severity {
        match self.limit(budget) {
            BudgetLimit::Flag { when_on } => when_on,
            _ => unreachable!("{budget} is a limit, not a flag"),
        }
    }

    fn limit(&self, budget: BudgetId) -> BudgetLimit {
        self.file
            .and_then(|file| file.limit(budget))
            .or_else(|| DEFAULT_BUDGET.limit(budget))
            .unwrap_or_else(|| unreachable!("the default budget sets {budget}"))
    }
}

/// The budget rule (design section 9.2). Fail or warn when the value is above that limit. Else
/// incomplete when some files have no estimate, because the value is then a lower bound. Else
/// pass.
fn budget_outcome(thresholds: Thresholds, value: u64, not_covered: &[RelPath]) -> Outcome {
    match thresholds.level(value) {
        Some(level) => Outcome::Above(level),
        None if not_covered.is_empty() => Outcome::Pass,
        None => Outcome::Incomplete(format!(
            "no estimate for {}: {}",
            count(not_covered.len(), "file"),
            path_list(not_covered)
        )),
    }
}

/// The verdict a flag gives when it is on. `allow` gives none.
fn level_of(severity: Severity) -> Option<Level> {
    match severity {
        Severity::Allow => None,
        Severity::Warn => Some(Level::Warn),
        Severity::Fail => Some(Level::Fail),
    }
}

/// Why some silence went unchecked: sprite maps with no estimate, and scripts that did not parse
/// and may hold more maps. None when everything was checked.
fn unchecked_sprite_maps(audio: &AudioSummary, bundle: &BundleSummary) -> Option<String> {
    let unestimated: Vec<&RelPath> = audio
        .sprite_maps
        .iter()
        .filter(|map| map.status != MeasureStatus::Estimated)
        .map(|map| &map.source)
        .collect();
    let mut reasons = Vec::new();
    if !unestimated.is_empty() {
        reasons.push(format!(
            "no estimate for {}: {}",
            count(unestimated.len(), "sprite map"),
            path_list(&unestimated)
        ));
    }
    reasons.extend(unparsed_scripts(bundle, "sprite maps"));
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

/// "N scripts did not parse and may hold <what>: <paths>", or None when every script parsed.
fn unparsed_scripts(bundle: &BundleSummary, what: &str) -> Option<String> {
    let scripts = &bundle.invalid_scripts;
    (!scripts.is_empty()).then(|| {
        format!(
            "{} did not parse and may hold {what}: {}",
            count(scripts.len(), "script"),
            path_list(scripts)
        )
    })
}

/// The site with the largest canvas floor, the first on a tie.
fn largest_floor<'s>(sites: &[&'s RenderSite]) -> Option<&'s RenderSite> {
    let mut worst: Option<&RenderSite> = None;
    for &site in sites {
        let Some(canvas) = site.canvas else {
            continue;
        };
        let is_larger = worst
            .and_then(|largest| largest.canvas)
            .is_none_or(|largest| canvas.bytes > largest.bytes);
        if is_larger {
            worst = Some(site);
        }
    }
    worst
}

/// One `remove_duplicate` per copy the build does not name, when it names exactly one. Each
/// saves the file's bytes, and its share of the group's decoded bytes. Otherwise one
/// `keep_one_copy` for the group, named by its first path, which saves the whole group's waste:
/// deleting a copy by name could delete the one the game loads.
fn copy_removals(group: &DuplicateGroup) -> Vec<Fix> {
    if group.keep.is_none() {
        return vec![Fix {
            saves_decoded_bytes: Some(group.decoded_bytes_saved),
            ..fix(
                FixAction::KeepOneCopy,
                group.paths.first(),
                group.wasted_bytes,
            )
        }];
    }
    let extra_copies = extra_copies(group);
    let decoded_per_copy = group
        .decoded_bytes_saved
        .checked_div(extra_copies.len() as u64)
        .unwrap_or(0);
    extra_copies
        .into_iter()
        .map(|path| Fix {
            saves_decoded_bytes: Some(decoded_per_copy),
            ..fix(FixAction::RemoveDuplicate, Some(path), group.bytes)
        })
        .collect()
}

fn fix(action: FixAction, path: Option<&RelPath>, saves_bytes: u64) -> Fix {
    Fix {
        action,
        path: path.cloned(),
        saves_bytes,
        saves_decoded_bytes: None,
    }
}

/// The fixes' paths, largest saving first, then by path.
fn paths_by_saving(fixes: &[Fix]) -> Vec<RelPath> {
    let mut by_saving: Vec<(Reverse<u64>, &RelPath)> = fixes
        .iter()
        .filter_map(|fix| Some((Reverse(fix.saves_bytes), fix.path.as_ref()?)))
        .collect();
    by_saving.sort();
    by_saving
        .into_iter()
        .map(|(_, path)| path.clone())
        .collect()
}

fn silence_text(map: &SpriteMapSummary) -> String {
    let audio = map
        .audio
        .as_ref()
        .map_or_else(|| "its audio".to_owned(), ToString::to_string);
    format!(
        "{}% of {audio} is not covered by any sprite in {}. Trim the gaps to save up to {} MB.",
        tenths(u64::from(map.silence_bp), BASIS_POINTS_PER_PERCENT),
        map.source,
        mb(map.silence_bytes_48k)
    )
}

/// Up to `NAMED_PATHS` paths, then how many more there are.
fn path_list<P: Borrow<RelPath>>(paths: &[P]) -> String {
    let named: Vec<&str> = paths
        .iter()
        .take(NAMED_PATHS)
        .map(|path| path.borrow().as_str())
        .collect();
    let list = named.join(", ");
    match paths.len().saturating_sub(NAMED_PATHS) {
        0 => list,
        more => format!("{list} and {more} more"),
    }
}

/// "1 file" or "2 files".
fn count(amount: usize, noun: &str) -> String {
    match amount {
        1 => format!("1 {noun}"),
        amount => format!("{amount} {noun}s"),
    }
}

fn it_or_them(amount: usize) -> &'static str {
    if amount == 1 { "it" } else { "them" }
}

fn mb(bytes: u64) -> String {
    tenths(bytes, BYTES_PER_MB)
}

fn kb(bytes: u64) -> String {
    tenths(bytes, BYTES_PER_KB)
}

/// `value / unit` with one decimal, halves rounded up.
fn tenths(value: u64, unit: u64) -> String {
    let tenths = (u128::from(value) * 10 + u128::from(unit) / 2) / u128::from(unit);
    format!("{}.{}", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use dyno_schema::measure::Source;

    use super::*;

    #[test]
    fn rules_lists_every_static_rule() {
        let static_rules: Vec<RuleId> = RuleId::ALL
            .iter()
            .copied()
            .filter(|rule| rule.spec().source == Source::Static)
            .collect();
        assert_eq!(RULES, static_rules);
    }

    #[test]
    fn the_default_budget_sets_every_key() {
        for &budget in BudgetId::ALL {
            assert!(DEFAULT_BUDGET.limit(budget).is_some(), "{budget}");
        }
    }

    #[test]
    fn sizes_show_one_decimal_with_halves_rounded_up() {
        assert_eq!(mb(BYTES_PER_MB), "1.0");
        assert_eq!(mb(BYTES_PER_MB * 3 / 4), "0.8");
        assert_eq!(mb(BYTES_PER_MB * 3 / 4 - 1), "0.7");
        assert_eq!(kb(1_536), "1.5");
    }

    #[test]
    fn a_long_path_list_names_five_and_counts_the_rest() {
        let paths: Vec<RelPath> = ["a", "b", "c", "d", "e", "f", "g"]
            .iter()
            .map(|raw| RelPath::new(raw).unwrap())
            .collect();
        assert_eq!(path_list(&paths), "a, b, c, d, e and 2 more");
        assert_eq!(path_list(&paths[..2]), "a, b");
    }
}
