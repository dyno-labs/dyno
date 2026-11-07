//! The static scan: what dyno can tell from a build folder without running it.
//!
//! One module per measure family. Each has one entry function and a `MEASURES` list of the
//! measures it owns. The families run in the order of the data flow in design section 2: a family
//! reads the summaries of the ones before it, and the rules read them all. Every static value is
//! `estimated`, never `measured`: it is arithmetic on the supplied files, not a reading of iPhone
//! memory.
//!
//! The scan does not sort the result's lists or compute its hash. `CheckResult::new` in
//! `dyno-schema` does both.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dyno_schema::budget::Budget;
use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::BuildManifest;
use dyno_schema::measure::{Measure, MeasureStatus};
use dyno_schema::path::{RelPath, RelPathError};
use dyno_schema::profile::HostProfile;
use dyno_schema::rule::Finding;
use zip::result::ZipError;

use crate::model::{AudioSummary, MeasureWithoutValue, Summaries, TextureSummary};

pub mod audio;
pub mod bundle;
pub mod bytes;
pub mod duplicates;
pub mod inventory;
pub mod model;
pub mod rules;
mod sniff;
pub mod textures;

/// The output of a static scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticScan {
    /// The build's files and id.
    pub manifest: Option<BuildManifest>,
    /// One measure per static measure id.
    pub measures: Vec<Measure>,
    /// One finding per static rule.
    pub findings: Vec<Finding>,
}

/// Why a build could not be scanned.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("build {0:?} does not exist, or is not a folder or a .zip file")]
    NotABuild(PathBuf),
    #[error("cannot read {path:?}: {source}")]
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot read the zip {path:?}: {source}")]
    BadZip { path: PathBuf, source: ZipError },
    /// A zip entry outside the build, or a file name a result cannot hold.
    #[error("a path in the build cannot be used: {0}")]
    BadPath(#[from] RelPathError),
    #[error("the zip has two entries for {0}")]
    DuplicateZipEntry(RelPath),
    #[error("the zip has an entry name with a NUL byte: {0:?}")]
    NulInZipName(String),
    /// A zip that unpacks past the inventory's limits, as a zip bomb does.
    #[error("the zip {path:?} is too large to scan: {reason}")]
    ZipTooLarge { path: PathBuf, reason: String },
}

/// Why every static measure is unsupported when a check has no build folder.
pub const NEEDS_A_BUILD: &str = "needs a build folder or .zip";

/// Stands in when a measure with no value gives no reason.
const NO_VALUE: &str = "no value";

/// Every measure family's measures, in the order the scan runs the families.
pub const FAMILIES: &[&[MeasureId]] = &[
    inventory::MEASURES,
    bundle::MEASURES,
    textures::MEASURES,
    audio::MEASURES,
    bytes::MEASURES,
    duplicates::MEASURES,
];

/// Scans a build folder or a .zip of one, then checks the static rules against the budget file.
pub fn scan(
    build: &Path,
    host_profile: &HostProfile,
    budget: Option<&Budget>,
) -> Result<StaticScan, ScanError> {
    let inventory = inventory::scan(build)?;
    let build_files = &inventory.summary;
    let bundle = bundle::scan(build_files, host_profile);
    let textures = textures::scan(build_files, bundle.summary.engine);
    let audio = audio::scan(build_files, &bundle.summary.howl_sites, host_profile);
    let bytes_measures = bytes::scan(build_files);
    let decoded_bytes = decoded_bytes_by_path(&textures.summary, &audio.summary);
    let duplicates = duplicates::scan(build_files, &decoded_bytes);

    let measures: Vec<Measure> = [
        inventory.measures,
        bundle.measures,
        textures.measures,
        audio.measures,
        bytes_measures,
        duplicates.measures,
    ]
    .into_iter()
    .flatten()
    .collect();
    let summaries = Summaries {
        bundle: bundle.summary,
        textures: textures.summary,
        audio: audio.summary,
        duplicates: duplicates.summary,
        measures_without_value: measures_without_value(&measures),
    };
    Ok(StaticScan {
        manifest: inventory.summary.into_manifest(),
        measures,
        findings: rules::check(&summaries, budget),
    })
}

/// Every static measure unsupported, for a check with only a launch URL. None is required.
pub fn without_build() -> StaticScan {
    let measures = FAMILIES
        .iter()
        .flat_map(|family| family.iter())
        .map(|&id| Measure {
            required: false,
            reason: Some(NEEDS_A_BUILD.to_owned()),
            ..Measure::blank(id, MeasureStatus::Unsupported)
        })
        .collect();
    StaticScan {
        manifest: None,
        measures,
        findings: rules::without_build(),
    }
}

/// Each decoded file's bytes by path: GPU bytes for a texture, bytes at 48 kHz for audio. With
/// it, duplicates need no texture or audio types.
fn decoded_bytes_by_path(
    textures: &TextureSummary,
    audio: &AudioSummary,
) -> BTreeMap<RelPath, u64> {
    let texture_bytes = textures
        .files
        .iter()
        .map(|file| (file.path.clone(), file.gpu_bytes));
    let audio_bytes = audio
        .files
        .iter()
        .map(|file| (file.path.clone(), file.decoded_bytes_48k));
    texture_bytes.chain(audio_bytes).collect()
}

/// Every measure whose status gives no value, with its reason.
fn measures_without_value(measures: &[Measure]) -> Vec<MeasureWithoutValue> {
    measures
        .iter()
        .filter(|measure| {
            !matches!(
                measure.status,
                MeasureStatus::Estimated | MeasureStatus::Measured
            )
        })
        .map(|measure| MeasureWithoutValue {
            id: measure.id,
            reason: measure
                .reason
                .clone()
                .unwrap_or_else(|| NO_VALUE.to_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use dyno_schema::ids::RuleId;
    use dyno_schema::measure::{MeasureValue, NOT_BUILT_YET, Source};

    use crate::model::{DecodedAudio, TextureCost, saturating_i64};

    use super::*;

    const PAGE: &[u8] = b"<!doctype html>";
    const IMAGE: &[u8] = b"\x89PNG copy";

    fn static_ids() -> BTreeSet<&'static str> {
        MeasureId::ALL
            .iter()
            .filter(|id| id.spec().source == Source::Static)
            .map(|id| id.as_str())
            .collect()
    }

    fn measure(scan: &StaticScan, id: MeasureId) -> &Measure {
        scan.measures
            .iter()
            .find(|measure| measure.id == id)
            .expect("the scan gives every static measure")
    }

    /// A build folder with the same image twice.
    fn build_with_a_copy() -> tempfile::TempDir {
        let build = tempfile::tempdir().unwrap();
        fs::write(build.path().join("index.html"), PAGE).unwrap();
        fs::write(build.path().join("a.png"), IMAGE).unwrap();
        fs::write(build.path().join("b.png"), IMAGE).unwrap();
        build
    }

    #[test]
    fn the_families_own_every_static_measure_once() {
        let owned: Vec<&str> = FAMILIES
            .iter()
            .flat_map(|family| family.iter())
            .map(|id| id.as_str())
            .collect();
        let unique: BTreeSet<&str> = owned.iter().copied().collect();
        assert_eq!(
            owned.len(),
            unique.len(),
            "a measure is in two families: {owned:?}"
        );
        assert_eq!(unique, static_ids());
    }

    #[test]
    fn a_scan_gives_every_static_measure_once() {
        let build = tempfile::tempdir().unwrap();
        let scan = scan(build.path(), &HostProfile::reels(), None).unwrap();
        let ids: BTreeSet<&str> = scan
            .measures
            .iter()
            .map(|measure| measure.id.as_str())
            .collect();
        assert_eq!(ids, static_ids());
        assert_eq!(scan.measures.len(), ids.len());
    }

    #[test]
    fn a_scan_gives_one_finding_per_static_rule() {
        let build = tempfile::tempdir().unwrap();
        let scan = scan(build.path(), &HostProfile::reels(), None).unwrap();
        let rule_ids: Vec<RuleId> = scan.findings.iter().map(|finding| finding.rule).collect();
        assert_eq!(rule_ids, rules::RULES);
    }

    #[test]
    fn a_scan_lists_the_build_in_its_manifest() {
        let build = build_with_a_copy();
        let scan = scan(build.path(), &HostProfile::reels(), None).unwrap();
        let manifest = scan.manifest.expect("a scanned build has a manifest");
        let paths: Vec<&str> = manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, ["a.png", "b.png", "index.html"]);
        assert_eq!(
            manifest.build_id,
            Some(inventory::build_id(&manifest.files))
        );
    }

    #[test]
    fn the_inventory_reaches_the_later_families() {
        let build = build_with_a_copy();
        let scan = scan(build.path(), &HostProfile::reels(), None).unwrap();
        let total_bytes = (PAGE.len() + 2 * IMAGE.len()) as u64;
        assert_eq!(
            measure(&scan, MeasureId::StaticBytesByType).value,
            Some(MeasureValue::Integer(saturating_i64(total_bytes)))
        );
        let wasted_bytes = IMAGE.len() as u64;
        assert_eq!(
            measure(&scan, MeasureId::StaticBytesDuplicates).value,
            Some(MeasureValue::Integer(saturating_i64(wasted_bytes)))
        );
    }

    #[test]
    fn only_measures_without_a_value_are_listed() {
        let estimated = Measure {
            value: Some(MeasureValue::Integer(1)),
            ..Measure::blank(MeasureId::StaticInventoryFiles, MeasureStatus::Estimated)
        };
        let measured = Measure {
            value: Some(MeasureValue::Integer(1)),
            ..Measure::blank(MeasureId::StaticBytesByType, MeasureStatus::Measured)
        };
        let missing = Measure::missing(MeasureId::StaticAudioDecoded48k, NOT_BUILT_YET);
        let unsupported = Measure {
            reason: Some("canvas2d".to_owned()),
            ..Measure::blank(MeasureId::StaticRenderCanvas, MeasureStatus::Unsupported)
        };
        let invalid = Measure::blank(MeasureId::StaticTexturesDecoded, MeasureStatus::Invalid);
        let listed = measures_without_value(&[estimated, measured, missing, unsupported, invalid]);
        let expected = [
            (MeasureId::StaticAudioDecoded48k, NOT_BUILT_YET),
            (MeasureId::StaticRenderCanvas, "canvas2d"),
            (MeasureId::StaticTexturesDecoded, NO_VALUE),
        ];
        let listed: Vec<(MeasureId, &str)> = listed
            .iter()
            .map(|without_value| (without_value.id, without_value.reason.as_str()))
            .collect();
        assert_eq!(listed, expected);
    }

    #[test]
    fn a_path_that_is_not_a_build_is_rejected() {
        let folder = tempfile::tempdir().unwrap();
        let text_file = folder.path().join("notes.txt");
        fs::write(&text_file, "x").unwrap();
        for path in [folder.path().join("absent"), text_file] {
            assert!(
                matches!(
                    scan(&path, &HostProfile::reels(), None),
                    Err(ScanError::NotABuild(_))
                ),
                "{path:?}"
            );
        }
    }

    #[test]
    fn a_zip_file_is_a_build_whatever_the_case_of_its_extension() {
        let folder = tempfile::tempdir().unwrap();
        let zip = folder.path().join("build.ZIP");
        zip::ZipWriter::new(fs::File::create(&zip).unwrap())
            .finish()
            .unwrap();
        assert!(scan(&zip, &HostProfile::reels(), None).is_ok());
    }

    #[test]
    fn without_a_build_there_is_no_manifest() {
        assert_eq!(without_build().manifest, None);
    }

    #[test]
    fn without_a_build_there_is_one_finding_per_static_rule() {
        let rule_ids: Vec<RuleId> = without_build()
            .findings
            .iter()
            .map(|finding| finding.rule)
            .collect();
        assert_eq!(rule_ids, rules::RULES);
    }

    #[test]
    fn without_a_build_every_static_measure_is_unsupported_and_optional() {
        let scan = without_build();
        assert_eq!(scan.measures.len(), static_ids().len());
        for measure in &scan.measures {
            assert_eq!(measure.status, MeasureStatus::Unsupported);
            assert!(!measure.required);
        }
    }

    #[test]
    fn decoded_bytes_come_from_textures_and_audio() {
        let path = |raw: &str| RelPath::new(raw).unwrap();
        let textures = TextureSummary {
            files: vec![TextureCost {
                path: path("a.png"),
                gpu_bytes: 4,
                astc_bytes: 1,
            }],
            ..TextureSummary::default()
        };
        let audio = AudioSummary {
            files: vec![DecodedAudio {
                path: path("b.ogg"),
                decoded_bytes_48k: 8,
            }],
            ..AudioSummary::default()
        };
        let decoded_bytes = decoded_bytes_by_path(&textures, &audio);
        assert_eq!(
            decoded_bytes,
            BTreeMap::from([(path("a.png"), 4), (path("b.ogg"), 8)])
        );
    }
}
