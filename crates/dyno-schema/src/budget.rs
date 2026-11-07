//! The budget file and the verdicts it gives.
//!
//! A budget file holds asset-cost budgets per part: decoded audio, decoded textures, and the
//! antialias and resolution flags. It also holds the thresholds of the static rules, which tune
//! their findings. It is TOML, so each line can say why. JSON also works. Unknown keys are an
//! error.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::canonical;
use crate::config::{self, ConfigFormat};
use crate::ids::{BudgetId, MeasureId};
use crate::measure::MeasureValue;
use crate::status::Verdict;

/// A budget file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// The file format version. Must equal the schema version this dyno reads.
    pub schema_version: u32,
    /// A name to recognise the file by. The result records it.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub audio: AudioBudget,
    #[serde(default)]
    pub textures: TextureBudget,
    #[serde(default)]
    pub render: RenderBudget,
    #[serde(default)]
    pub files: FilesBudget,
}

/// Audio budgets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioBudget {
    /// Decoded audio at 48 kHz, in MB.
    #[serde(default)]
    pub decoded_mb: Option<SizeLimit>,
    /// The `audio.stereo` rule's threshold: decoded MB at 48 kHz that a mono export of every
    /// stereo sound would save.
    #[serde(default)]
    pub stereo_saving_mb: Option<SizeLimit>,
    /// The `audio.silence_share` rule's threshold on a sprite map's share of time that no sprite
    /// covers. A map is over it only when it is also over `silence_saving_mb`.
    #[serde(default)]
    pub silence_share_bp: Option<ShareLimit>,
    /// The `audio.silence_share` rule's threshold on the decoded MB at 48 kHz in a sprite map's
    /// uncovered time.
    #[serde(default)]
    pub silence_saving_mb: Option<SizeLimit>,
}

/// Texture budgets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextureBudget {
    /// Decoded textures, one copy, in MB.
    #[serde(default)]
    pub decoded_mb: Option<SizeLimit>,
}

/// Render config budgets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderBudget {
    /// The verdict when the bundle turns antialias on.
    #[serde(default)]
    pub antialias: Option<Severity>,
    /// The verdict when the bundle leaves the resolution uncapped.
    #[serde(default)]
    pub resolution_uncapped: Option<Severity>,
}

/// Rule thresholds for the build's files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilesBudget {
    /// The `files.duplicate` rule's outcome when two or more files have the same SHA-256.
    #[serde(default)]
    pub duplicate: Option<Severity>,
}

/// A size limit in whole MB. A value above `warn` warns, and a value above `fail` fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SizeLimit {
    #[serde(default)]
    pub warn: Option<u32>,
    #[serde(default)]
    pub fail: Option<u32>,
}

/// A share limit in basis points: 10,000 is 100%. A value above `warn` warns, and a value above
/// `fail` fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareLimit {
    #[serde(default)]
    pub warn: Option<u32>,
    #[serde(default)]
    pub fail: Option<u32>,
}

/// The verdict a flag gives when it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Allow,
    Warn,
    Fail,
}

/// Which budget file a result used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct BudgetRef {
    /// The file's `name`, if it has one.
    pub name: Option<String>,
    /// Lowercase hex SHA-256 of the parsed budgets as canonical JSON. The same budgets in TOML
    /// and JSON give the same hash, and comments do not change it.
    pub sha256: String,
}

/// The verdict of one budget. Only budgets that `dyno-score` checks get one, never a rule's
/// threshold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct BudgetVerdict {
    pub budget: BudgetId,
    pub measure: MeasureId,
    /// A partial value, one whose scope has a `not_covered` entry, is a floor. It gives a verdict
    /// only when the full value could not give a worse one. Otherwise the verdict is incomplete.
    pub verdict: Verdict,
    /// The measure's value. Null when it has no usable value: none, a status other than measured
    /// or estimated, or a bad number.
    pub actual: Option<MeasureValue>,
    pub limit: BudgetLimit,
}

/// A budget's limit as a verdict or rule reads it: sizes in bytes, shares in basis points, and
/// flags as a severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BudgetLimit {
    /// Byte limits. A value above `warn_above` warns, and a value above `fail_above` fails.
    Bytes {
        warn_above: Option<u64>,
        fail_above: Option<u64>,
    },
    /// The verdict when the flag is on.
    Flag { when_on: Severity },
    /// Share limits in basis points. A value above `warn_above` warns, and a value above
    /// `fail_above` fails.
    BasisPoints {
        warn_above: Option<u64>,
        fail_above: Option<u64>,
    },
}

/// Why a budget file was rejected.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("budget file {0}")]
pub struct BudgetError(String);

/// One MB in a budget file: 1,048,576 bytes, the unit the plan's numbers use.
pub const BYTES_PER_MB: u64 = 1_048_576;

/// A whole share in basis points: 100%.
const WHOLE_SHARE_BP: u32 = 10_000;

impl Budget {
    /// Reads a budget file. A `.json` file is JSON. Anything else is TOML.
    pub fn load(path: &Path) -> Result<Budget, BudgetError> {
        let budget: Budget = config::load(path).map_err(BudgetError)?;
        budget
            .validate()
            .map_err(|problem| BudgetError(format!("{}: {problem}", path.display())))?;
        Ok(budget)
    }

    /// Parses a budget file's text.
    pub fn parse(text: &str, format: ConfigFormat) -> Result<Budget, BudgetError> {
        let budget: Budget = config::parse(text, format).map_err(BudgetError)?;
        budget.validate().map_err(BudgetError)?;
        Ok(budget)
    }

    /// The limit this file sets for one budget: bytes, basis points or a flag's severity.
    pub fn limit(&self, budget: BudgetId) -> Option<BudgetLimit> {
        match budget {
            BudgetId::AudioDecodedMb => self.audio.decoded_mb.map(SizeLimit::in_bytes),
            BudgetId::AudioStereoSavingMb => self.audio.stereo_saving_mb.map(SizeLimit::in_bytes),
            BudgetId::AudioSilenceShareBp => {
                self.audio.silence_share_bp.map(ShareLimit::in_basis_points)
            }
            BudgetId::AudioSilenceSavingMb => self.audio.silence_saving_mb.map(SizeLimit::in_bytes),
            BudgetId::TexturesDecodedMb => self.textures.decoded_mb.map(SizeLimit::in_bytes),
            BudgetId::RenderAntialias => self
                .render
                .antialias
                .map(|when_on| BudgetLimit::Flag { when_on }),
            BudgetId::RenderResolutionUncapped => self
                .render
                .resolution_uncapped
                .map(|when_on| BudgetLimit::Flag { when_on }),
            BudgetId::FilesDuplicate => self
                .files
                .duplicate
                .map(|when_on| BudgetLimit::Flag { when_on }),
        }
    }

    /// The name and content hash a result records for this file.
    pub fn reference(&self) -> Result<BudgetRef, serde_json::Error> {
        Ok(BudgetRef {
            name: self.name.clone(),
            sha256: canonical::sha256_of_canonical_json(self)?,
        })
    }

    fn validate(&self) -> Result<(), String> {
        config::check_schema_version(self.schema_version)?;
        let size_limits = [
            (BudgetId::AudioDecodedMb, self.audio.decoded_mb),
            (BudgetId::AudioStereoSavingMb, self.audio.stereo_saving_mb),
            (BudgetId::AudioSilenceSavingMb, self.audio.silence_saving_mb),
            (BudgetId::TexturesDecodedMb, self.textures.decoded_mb),
        ];
        for (budget, limit) in size_limits {
            if let Some(limit) = limit {
                limit
                    .validate()
                    .map_err(|problem| format!("{budget}: {problem}"))?;
            }
        }
        if let Some(limit) = self.audio.silence_share_bp {
            limit
                .validate()
                .map_err(|problem| format!("{}: {problem}", BudgetId::AudioSilenceShareBp))?;
        }
        Ok(())
    }
}

impl SizeLimit {
    fn in_bytes(self) -> BudgetLimit {
        let to_bytes = |mb: u32| u64::from(mb) * BYTES_PER_MB;
        BudgetLimit::Bytes {
            warn_above: self.warn.map(to_bytes),
            fail_above: self.fail.map(to_bytes),
        }
    }

    fn validate(self) -> Result<(), String> {
        validate_warn_and_fail(self.warn, self.fail, "MB")
    }
}

impl ShareLimit {
    fn in_basis_points(self) -> BudgetLimit {
        BudgetLimit::BasisPoints {
            warn_above: self.warn.map(u64::from),
            fail_above: self.fail.map(u64::from),
        }
    }

    fn validate(self) -> Result<(), String> {
        validate_warn_and_fail(self.warn, self.fail, "bp")?;
        if [self.warn, self.fail]
            .into_iter()
            .flatten()
            .any(|limit| limit >= WHOLE_SHARE_BP)
        {
            return Err(format!("limits must be below {WHOLE_SHARE_BP} bp (100%)"));
        }
        Ok(())
    }
}

/// Checks a warn and fail pair in any unit: at least one is set, both are above 0, and warn is
/// below fail.
fn validate_warn_and_fail(warn: Option<u32>, fail: Option<u32>, unit: &str) -> Result<(), String> {
    match (warn, fail) {
        (None, None) => Err("set warn, fail or both".to_owned()),
        (Some(0), _) | (_, Some(0)) => Err(format!("limits must be above 0 {unit}")),
        (Some(warn), Some(fail)) if warn >= fail => Err(format!(
            "warn ({warn} {unit}) must be below fail ({fail} {unit})"
        )),
        _ => Ok(()),
    }
}
