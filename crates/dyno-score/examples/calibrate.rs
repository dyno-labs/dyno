//! Reads a calibration file and prints the error report of both recipes as JSON (docs/calibration.md):
//!
//! ```sh
//! cargo run -p dyno-score --example calibrate -- path/to/calibration.toml
//! ```
//!
//! The results it names are `dyno check` results of the build and its launch URL.

use std::path::Path;
use std::process::ExitCode;

use dyno_schema::result::LoadedResult;
use dyno_score::calibration::{self, CalibrationFile, Recipe, Row};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: calibrate <calibration.toml>");
        return ExitCode::from(64);
    };
    match reports(Path::new(&path)) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("calibrate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn reports(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let file: CalibrationFile = toml::from_str(&text).map_err(|error| error.to_string())?;
    let folder = path.parent().unwrap_or(Path::new("."));
    let mut reports = Vec::new();
    for recipe in [Recipe::Static, Recipe::Lab] {
        let rows = file
            .games
            .iter()
            .map(|game| {
                let result = read_result(&folder.join(&game.result))?;
                let parts = calibration::parts(&result.result, recipe)
                    .map_err(|error| format!("{}: {error}", game.id))?;
                calibration::row(game, &file.device, parts)
            })
            .collect::<Result<Vec<Row>, String>>()?;
        reports.push(calibration::report(recipe, &rows, &file));
    }
    serde_json::to_string_pretty(&reports).map_err(|error| error.to_string())
}

fn read_result(path: &Path) -> Result<LoadedResult, String> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).map_err(|error| format!("{shown}: {error}"))?;
    let json = serde_json::from_str(&text).map_err(|error| format!("{shown}: {error}"))?;
    LoadedResult::from_json(json).map_err(|error| format!("{shown}: {error}"))
}
