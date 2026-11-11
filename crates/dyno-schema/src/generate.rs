//! The generated files: JSON Schema and the measure and device catalogues in
//! `packages/schema/json/`, and TypeScript types in `packages/schema/ts/`, the two halves of the
//! npm package `@dyno-cloud/schema`.
//!
//! Tests compare these with the committed files and fail when they are stale. To update the
//! committed files, run `DYNO_UPDATE_GENERATED=1 INSTA_UPDATE=always cargo test --workspace`.

use std::path::Path;

use schemars::{JsonSchema, schema_for};
use serde::Serialize;
use ts_rs::{Config, ExportError, TS};

use crate::budget::Budget;
use crate::catalogue::{CatalogueResult, GamesFile};
use crate::compare::CompareResult;
use crate::devices::{DeviceEntry, devices};
use crate::doctor::DoctorReport;
use crate::host::{ExposureCheckpoint, UncleanExit};
use crate::ids::MeasureEntry;
use crate::library::{Library, LibraryBucket};
use crate::link::LinkFile;
use crate::probe::ProbeEvent;
use crate::result::CheckResult;
use crate::runner::{DriverMessage, RunRequest};

/// The environment variable that makes the generated-file tests rewrite the committed files,
/// when it is set to 1.
pub const UPDATE_GENERATED_VARIABLE: &str = "DYNO_UPDATE_GENERATED";

/// Each file this crate owns in `packages/schema/json/`, by name, with its content: the JSON
/// Schema files and the catalogues.
pub fn json_files() -> Vec<(&'static str, String)> {
    let mut files = json_schemas();
    files.extend(json_catalogues());
    files
}

/// The catalogues a site reads: every measure with its spec, and every device dyno knows.
pub fn json_catalogues() -> Vec<(&'static str, String)> {
    vec![
        ("devices.json", pretty_json(&devices())),
        ("measures.json", pretty_json(&MeasureEntry::all())),
    ]
}

/// Each JSON Schema file this crate owns: its name in `packages/schema/json/` and its content.
pub fn json_schemas() -> Vec<(&'static str, String)> {
    vec![
        ("budget.schema.json", pretty_schema::<Budget>()),
        ("catalogue.schema.json", pretty_schema::<CatalogueResult>()),
        ("compare.schema.json", pretty_schema::<CompareResult>()),
        ("doctor.schema.json", pretty_schema::<DoctorReport>()),
        ("dyno-toml.schema.json", pretty_schema::<LinkFile>()),
        (
            "driver-message.schema.json",
            pretty_schema::<DriverMessage>(),
        ),
        (
            "exposure-checkpoint.schema.json",
            pretty_schema::<ExposureCheckpoint>(),
        ),
        ("games.schema.json", pretty_schema::<GamesFile>()),
        ("library.schema.json", pretty_schema::<Library>()),
        (
            "library-bucket.schema.json",
            pretty_schema::<LibraryBucket>(),
        ),
        ("probe-event.schema.json", pretty_schema::<ProbeEvent>()),
        ("result.schema.json", pretty_schema::<CheckResult>()),
        ("run-request.schema.json", pretty_schema::<RunRequest>()),
        ("unclean-exit.schema.json", pretty_schema::<UncleanExit>()),
    ]
}

/// Pretty JSON Schema for one type, with a final newline.
pub fn pretty_schema<T: JsonSchema>() -> String {
    pretty_json(&schema_for!(T))
}

/// Pretty JSON, with a final newline.
fn pretty_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("the generated files always serialize") + "\n"
}

/// Writes one TypeScript file per type into `dir`, for every output shape, the runner protocol,
/// the host records and the rows of the catalogues. The budget, games and link files are TOML
/// that people write, so they get JSON Schema only.
///
/// Sizes and counts are `number`, because `JSON.parse` gives numbers. `to_canonical_json` rejects
/// any integer outside the safe integer range, so a `number` holds every one exactly.
pub fn export_typescript(dir: &Path) -> Result<(), ExportError> {
    let config = Config::new().with_large_int("number").with_out_dir(dir);
    CatalogueResult::export_all(&config)?;
    CheckResult::export_all(&config)?;
    CompareResult::export_all(&config)?;
    DeviceEntry::export_all(&config)?;
    DoctorReport::export_all(&config)?;
    DriverMessage::export_all(&config)?;
    ExposureCheckpoint::export_all(&config)?;
    Library::export_all(&config)?;
    LibraryBucket::export_all(&config)?;
    MeasureEntry::export_all(&config)?;
    ProbeEvent::export_all(&config)?;
    RunRequest::export_all(&config)?;
    UncleanExit::export_all(&config)?;
    write_typescript_index(dir)?;
    Ok(())
}

/// The file in `packages/schema/ts/` that re-exports every type, so the npm package has one entry
/// point.
pub const TYPESCRIPT_INDEX: &str = "index.ts";

/// Writes `index.ts`: one `export type *` line per type file in `dir`, sorted by name.
fn write_typescript_index(dir: &Path) -> Result<(), ExportError> {
    let mut modules = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let file_name = entry?.file_name().to_string_lossy().into_owned();
        if let Some(module) = file_name.strip_suffix(".ts")
            && file_name != TYPESCRIPT_INDEX
        {
            modules.push(module.to_owned());
        }
    }
    modules.sort();
    let mut index = String::from("// Generated by dyno-schema. Do not edit this file manually.\n");
    for module in modules {
        index.push_str(&format!("export type * from \"./{module}\";\n"));
    }
    std::fs::write(dir.join(TYPESCRIPT_INDEX), index)?;
    Ok(())
}
