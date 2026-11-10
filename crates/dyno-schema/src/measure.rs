//! One measure: a number or fact about the game, with its status, unit, evidence, scope and
//! uncertainty.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::canonical::{is_safe_integer, outside_the_safe_range};
use crate::ids::{MeasureId, Phase};

/// One measure in a result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Measure {
    pub id: MeasureId,
    pub status: MeasureStatus,
    pub source: Source,
    /// The plan phase that builds this measure: 1 or 2.
    pub phase: Phase,
    /// Whether the result needs this measure. A required measure that is missing or invalid
    /// stops the result from passing. Phase 2 measures are required only when a flag asks for
    /// them.
    pub required: bool,
    pub unit: Unit,
    /// The value when the status is measured or estimated. An invalid measure may keep the value
    /// that failed its check. Null otherwise.
    pub value: Option<MeasureValue>,
    /// A breakdown of the value, such as one item per file. Sorted by key, then value.
    pub items: Vec<MeasureItem>,
    /// How the value was found. Sorted.
    pub evidence: Vec<Evidence>,
    /// What the value covers, and what it does not.
    pub scope: Scope,
    /// How close the value is to the truth.
    pub uncertainty: Uncertainty,
    /// Why the status is missing, unsupported or invalid. Null otherwise.
    pub reason: Option<String>,
}

/// How far to trust a measure. "Not measured" is never 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum MeasureStatus {
    /// Observed directly in a browser run or on a phone.
    Measured,
    /// Computed from the build's files or from the model. Never a measure of iPhone memory.
    Estimated,
    /// dyno cannot measure this for this target, for example bytes to playable when the game
    /// sends no playable hook.
    Unsupported,
    /// There is no value.
    Missing,
    /// A value failed a check, for example a frame DPR that is not 3. The value stays, unless it
    /// was outside the safe integer range.
    Invalid,
}

/// Where a value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Arithmetic on the build's files. The same on every machine.
    Static,
    /// A Chromium run in the harness page.
    Lab,
    /// A run on a real iPhone.
    Device,
    /// The fitted memory model in `dyno-score`.
    Model,
    /// The injected field probe or the host script.
    Field,
}

/// The unit of a value. Sizes are always integer bytes. Only a change in size can be negative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// A size.
    Bytes,
    /// A change in size, such as growth per 100 spins. It is negative when memory shrinks.
    BytesChange,
    /// Milliseconds.
    Ms,
    /// A number of things.
    Count,
    /// Times a second, such as a frame rate or an audio sample rate.
    Hertz,
    /// Hundredths of a percent: 10,000 is 100%.
    BasisPoints,
    /// Thousandths: a device pixel ratio of 3 is 3,000.
    Milli,
    /// True or false.
    Flag,
    /// A string, such as a renderer name.
    Text,
}

/// A value. Integers carry every number, so float formatting never changes the output. They are
/// signed, because a change such as memory growth can be negative.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum MeasureValue {
    Integer(i64),
    Flag(bool),
    Text(String),
}

/// One part of a measure's breakdown. Items sort by key, then value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
pub struct MeasureItem {
    /// A relative path with '/', a file kind, a sprite name, an engine name or a URL.
    pub key: String,
    /// In the measure's unit, or text for a fact such as an engine version.
    pub value: MeasureValue,
}

/// One piece of evidence for a value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
pub struct Evidence {
    pub method: Method,
    /// A short plain-English note, such as the files read.
    pub detail: String,
}

/// How a value was found. Declared in alphabetical order, so evidence sorts by name.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Decoding a sound's samples.
    AudioDecode,
    /// A scan of the JavaScript bundle.
    BundleScan,
    /// A Chrome DevTools Protocol reading, such as the JS heap.
    DevtoolsProtocol,
    /// A file's SHA-256.
    FileHash,
    /// A file's header, such as image dimensions or audio duration.
    FileHeader,
    /// A file's size.
    FileSize,
    /// A postMessage between the game frame and the host page.
    HostMessage,
    /// The fitted memory model.
    Model,
    /// Network events on the game frame's own session.
    Network,
    /// An event from the in-page probe.
    Probe,
    /// A Howler sprite map.
    SpriteMap,
    /// The iPhone syslog.
    Syslog,
    /// The iPhone's sysmontap samples, which Instruments' Activity Monitor reads too.
    Sysmontap,
    /// An xctrace recording on the iPhone. Device runs of older dyno versions have it.
    Xctrace,
}

/// What a value covers, and what it does not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Scope {
    pub covers: Covers,
    /// Plain-English parts the value leaves out, such as "a.ogg: did not decode". Sorted. Any
    /// entry makes the value partial, so a budget reads it as a floor.
    pub not_covered: Vec<String>,
}

/// The set of things a value covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Covers {
    /// Every file in the build. An inventory, not an upper bound on memory.
    SuppliedFiles,
    /// One load of the game in the harness page, until idle.
    LoadToIdle,
    /// Spins in a demo-only session.
    DemoSpins,
    /// Fresh-process runs on the reference iPhone.
    FreshDeviceRuns,
    /// Whatever the model's static and lab inputs cover.
    ModelInputs,
}

/// How close a value is to the truth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct Uncertainty {
    pub kind: UncertaintyKind,
    /// The lowest likely value, in the measure's unit.
    pub low: Option<i64>,
    /// The highest likely value, in the measure's unit.
    pub high: Option<i64>,
    /// A short plain-English note, such as "0 to 23% below the decoded size".
    pub note: Option<String>,
}

/// The kind of uncertainty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyKind {
    /// Exact for what it covers, such as bytes on disk.
    Exact,
    /// The true value is at or above the value.
    Floor,
    /// The true value is between low and high.
    Range,
    /// The model's published error applies.
    ModelError,
    /// Not known, for example because the value is missing.
    Unknown,
}

/// The reason a stub gives for a measure, rule or check that no step has built yet.
pub const NOT_BUILT_YET: &str = "not built yet";

impl Measure {
    /// A measure with no value, evidence or reason. The source, phase, unit and scope come from
    /// the id. Phase 1 measures are required, but only as a default: the producer changes
    /// `required` where the check's inputs say otherwise, as for a check with no build.
    pub fn blank(id: MeasureId, status: MeasureStatus) -> Measure {
        let spec = id.spec();
        Measure {
            id,
            status,
            source: spec.source,
            phase: spec.phase,
            required: spec.phase == 1,
            unit: spec.unit,
            value: None,
            items: Vec::new(),
            evidence: Vec::new(),
            scope: Scope {
                covers: spec.covers,
                not_covered: Vec::new(),
            },
            uncertainty: Uncertainty::unknown(),
            reason: None,
        }
    }

    /// A measure with no value, and the reason why.
    pub fn missing(id: MeasureId, reason: &str) -> Measure {
        Measure {
            reason: Some(reason.to_owned()),
            ..Measure::blank(id, MeasureStatus::Missing)
        }
    }

    /// Sorts every list in the measure by a complete key.
    pub fn sort_lists(&mut self) {
        self.items.sort();
        self.evidence.sort();
        self.scope.not_covered.sort();
    }

    /// Why the first bad number in the value, the items or the uncertainty bounds is bad: an
    /// integer outside the safe integer range, or a negative number in a unit that is never
    /// negative. None when every number is fine.
    pub fn bad_number_reason(&self) -> Option<String> {
        self.integers().find_map(|(field, integer)| {
            if !is_safe_integer(integer) {
                Some(outside_the_safe_range(field, integer))
            } else if integer < 0 && self.unit.is_never_negative() {
                Some(format!(
                    "{field} {integer} is negative, but only a change in size can be negative"
                ))
            } else {
                None
            }
        })
    }

    /// Makes the measure invalid when it has a bad number, and says why in its reason.
    ///
    /// Integers outside the safe range are dropped, because the hash cannot tell them apart. The
    /// reason names the first bad number. An unsafe value or bound also resets the whole
    /// uncertainty to unknown, both bounds included, because its kind no longer holds. A negative
    /// number stays, as the value that failed its check.
    pub fn invalidate_bad_numbers(&mut self) {
        let Some(reason) = self.bad_number_reason() else {
            return;
        };
        self.status = MeasureStatus::Invalid;
        self.reason = Some(reason);
        self.items.retain(|item| !is_unsafe_integer(&item.value));
        let bounds = [self.uncertainty.low, self.uncertainty.high];
        let unsafe_bound = bounds
            .into_iter()
            .flatten()
            .any(|bound| !is_safe_integer(bound));
        let unsafe_value = self.value.as_ref().is_some_and(is_unsafe_integer);
        if unsafe_value {
            self.value = None;
        }
        if unsafe_value || unsafe_bound {
            self.uncertainty = Uncertainty::unknown();
        }
    }

    /// Every integer in the measure, with the field it is in.
    fn integers(&self) -> impl Iterator<Item = (&'static str, i64)> + '_ {
        let value = self.value.iter().map(|value| ("value", value));
        let items = self.items.iter().map(|item| ("item value", &item.value));
        let values = value.chain(items).filter_map(|(field, value)| match value {
            MeasureValue::Integer(integer) => Some((field, *integer)),
            MeasureValue::Flag(_) | MeasureValue::Text(_) => None,
        });
        let low = self.uncertainty.low.map(|low| ("low bound", low));
        let high = self.uncertainty.high.map(|high| ("high bound", high));
        values.chain(low).chain(high)
    }
}

impl Unit {
    /// Whether a value in this unit is never negative: true for every unit but a change in size.
    /// Flags and text have no sign.
    pub fn is_never_negative(self) -> bool {
        self != Unit::BytesChange
    }
}

impl Uncertainty {
    /// The value is exact for what it covers, such as bytes on disk.
    pub fn exact() -> Uncertainty {
        Uncertainty {
            kind: UncertaintyKind::Exact,
            low: None,
            high: None,
            note: None,
        }
    }

    /// Nothing is known about how close the value is.
    pub fn unknown() -> Uncertainty {
        Uncertainty {
            kind: UncertaintyKind::Unknown,
            low: None,
            high: None,
            note: None,
        }
    }
}

/// One missing measure per id, each with the same reason.
pub fn missing_measures(ids: &[MeasureId], reason: &str) -> Vec<Measure> {
    ids.iter().map(|&id| Measure::missing(id, reason)).collect()
}

/// Whether the value is an integer outside the safe range. A flag or text never is.
fn is_unsafe_integer(value: &MeasureValue) -> bool {
    matches!(value, MeasureValue::Integer(integer) if !is_safe_integer(*integer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::MAX_SAFE_INTEGER;

    /// The smallest integer above the safe range: 2^53.
    const FIRST_UNSAFE: i64 = MAX_SAFE_INTEGER + 1;

    fn estimated_bytes(value: i64) -> Measure {
        Measure {
            value: Some(MeasureValue::Integer(value)),
            ..Measure::blank(MeasureId::StaticTexturesDecoded, MeasureStatus::Estimated)
        }
    }

    fn item(key: &str, value: i64) -> MeasureItem {
        MeasureItem {
            key: key.to_owned(),
            value: MeasureValue::Integer(value),
        }
    }

    fn a_range(low: i64, high: i64) -> Uncertainty {
        Uncertainty {
            kind: UncertaintyKind::Range,
            low: Some(low),
            high: Some(high),
            note: None,
        }
    }

    /// Growth per 100 spins that shrank, with a range on both sides of 0.
    fn shrinking_growth() -> Measure {
        Measure {
            value: Some(MeasureValue::Integer(-4_096)),
            uncertainty: a_range(-8_192, 1_024),
            ..Measure::blank(MeasureId::PlayGrowthPer100Spins, MeasureStatus::Measured)
        }
    }

    #[test]
    fn methods_are_declared_in_json_name_order() {
        let methods = [
            Method::BundleScan,
            Method::DevtoolsProtocol,
            Method::FileHash,
            Method::FileHeader,
            Method::FileSize,
            Method::HostMessage,
            Method::Model,
            Method::Network,
            Method::Probe,
            Method::SpriteMap,
            Method::Syslog,
            Method::Sysmontap,
            Method::Xctrace,
        ];
        let names: Vec<String> = methods
            .iter()
            .map(|method| serde_json::to_string(method).unwrap())
            .collect();
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(names, sorted_names);
        assert!(methods.is_sorted());
    }

    #[test]
    fn blank_measures_take_their_metadata_from_the_id() {
        let measure = Measure::missing(MeasureId::PlaySpins, NOT_BUILT_YET);
        assert_eq!(measure.source, Source::Lab);
        assert_eq!(measure.phase, 2);
        assert!(!measure.required);
        assert_eq!(measure.unit, Unit::Count);
        assert_eq!(measure.scope.covers, Covers::DemoSpins);
        assert_eq!(measure.reason.as_deref(), Some(NOT_BUILT_YET));
    }

    #[test]
    fn growth_is_a_change_in_size() {
        assert_eq!(
            MeasureId::PlayGrowthPer100Spins.spec().unit,
            Unit::BytesChange
        );
    }

    #[test]
    fn a_negative_change_and_its_bounds_are_fine() {
        assert_eq!(shrinking_growth().bad_number_reason(), None);
    }

    #[test]
    fn negative_numbers_survive_a_json_round_trip() {
        let growth = shrinking_growth();
        let json = serde_json::to_string(&growth).unwrap();
        assert_eq!(serde_json::from_str::<Measure>(&json).unwrap(), growth);
    }

    #[test]
    fn safe_and_non_negative_numbers_are_fine() {
        let mut measure = Measure {
            uncertainty: a_range(0, MAX_SAFE_INTEGER),
            ..estimated_bytes(MAX_SAFE_INTEGER)
        };
        assert_eq!(measure.bad_number_reason(), None);
        measure.invalidate_bad_numbers();
        assert_eq!(measure.status, MeasureStatus::Estimated);
    }

    #[test]
    fn an_unsafe_value_makes_the_measure_invalid_and_is_dropped() {
        let mut measure = estimated_bytes(FIRST_UNSAFE + 1);
        measure.invalidate_bad_numbers();
        assert_eq!(measure.status, MeasureStatus::Invalid);
        assert_eq!(measure.value, None);
        let reason = measure.reason.unwrap();
        assert!(
            reason.starts_with("value 9007199254740993 is outside"),
            "{reason}"
        );
    }

    #[test]
    fn unsafe_items_are_dropped_and_safe_ones_stay() {
        let mut measure = estimated_bytes(1);
        measure.items = vec![item("a.png", 1), item("b.png", FIRST_UNSAFE)];
        measure.invalidate_bad_numbers();
        assert_eq!(measure.status, MeasureStatus::Invalid);
        assert_eq!(measure.value, Some(MeasureValue::Integer(1)));
        assert_eq!(measure.items, [item("a.png", 1)]);
        assert!(measure.reason.unwrap().starts_with("item value"));
    }

    #[test]
    fn dropping_the_value_or_a_bound_makes_the_uncertainty_unknown() {
        let unsafe_value = Measure {
            uncertainty: a_range(1, 2),
            ..estimated_bytes(FIRST_UNSAFE)
        };
        let unsafe_bound = Measure {
            uncertainty: a_range(1, FIRST_UNSAFE),
            ..estimated_bytes(1)
        };
        for mut measure in [unsafe_value, unsafe_bound] {
            measure.invalidate_bad_numbers();
            assert_eq!(measure.uncertainty, Uncertainty::unknown());
        }
    }

    #[test]
    fn a_negative_number_is_invalid_and_keeps_its_value() {
        // Every unit but a change in size. Flags and text hold no integer to check.
        let units = [
            Unit::Bytes,
            Unit::Count,
            Unit::Ms,
            Unit::Hertz,
            Unit::BasisPoints,
            Unit::Milli,
        ];
        for unit in units {
            let mut measure = Measure {
                unit,
                ..estimated_bytes(-1)
            };
            measure.invalidate_bad_numbers();
            assert_eq!(measure.status, MeasureStatus::Invalid, "{unit:?}");
            assert_eq!(measure.value, Some(MeasureValue::Integer(-1)));
            assert!(measure.reason.unwrap().contains("negative"));
        }
    }

    #[test]
    fn a_negative_bound_makes_the_measure_invalid_and_the_bound_is_kept() {
        let mut measure = Measure {
            uncertainty: a_range(-1, 2),
            ..estimated_bytes(1)
        };
        measure.invalidate_bad_numbers();
        assert_eq!(measure.status, MeasureStatus::Invalid);
        assert_eq!(measure.uncertainty, a_range(-1, 2));
        assert!(measure.reason.unwrap().starts_with("low bound -1"));
    }

    #[test]
    fn items_with_the_same_key_sort_by_value() {
        let mut measure = Measure::blank(MeasureId::StaticBundleEngines, MeasureStatus::Estimated);
        measure.items = vec![item("pixi", 2), item("howler", 1), item("pixi", 1)];
        measure.sort_lists();
        assert_eq!(
            measure.items,
            [item("howler", 1), item("pixi", 1), item("pixi", 2)]
        );
    }
}
