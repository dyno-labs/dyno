//! `dyno check`: one target, or every game in a games file.
//!
//! A check runs the static scan and its rules if it has a build, then the budgets, then a browser
//! run if it has a launch URL. The status and exit code come from the deterministic part
//! alone. [`check_target`] gives one result and writes nothing, so the MCP `check` tool calls it
//! too.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dyno_cdp::CdpDriver;
use dyno_run::Driver;
use dyno_run::exec::ExecDriver;
use dyno_schema::budget::{Budget, BudgetRef};
use dyno_schema::catalogue::{CatalogueEntry, CatalogueResult, GameEntry, GamesFile, Shard};
use dyno_schema::launch_url::LaunchUrl;
use dyno_schema::profile::{HostProfile, Scenario};
use dyno_schema::result::{CheckResult, Deterministic, Inputs, Release, RunInfo, Target, Versions};
use dyno_schema::runner::{RUNNER_PROTOCOL_VERSION, RunRequest};
use dyno_schema::status::Status;
use dyno_schema::{SCHEMA_VERSION, TOOL_VERSION};
use dyno_score::SCORING_VERSION;
use dyno_score::budget::budget_verdicts;
use dyno_score::model::{MODEL_VERSION, model_measures, provisional_tier};
use dyno_score::status::{overall_status, run_coverage, worst};
use dyno_serve::{DEFAULT_PORT, DEFAULT_WAIT_FOR_BROWSER, ServeDriver, ServeError, ServeOptions};

use crate::cli::{CheckArgs, DriverArg};
use crate::device;
use crate::exit::CliError;
use crate::output::{self, ResultPlace};
use crate::run_info::{Clock, machine, tool_build};

/// What one check looks at.
#[derive(Debug, Default)]
pub struct CheckTarget {
    /// A build folder or .zip.
    build: Option<PathBuf>,
    /// The full launch URL, query string included.
    launch_url: Option<LaunchUrl>,
}

/// The flags that apply to every game in a check.
pub struct CheckOptions {
    host_profile: HostProfile,
    budget: Option<Budget>,
    /// The budget file's name and hash, computed once for every game.
    budget_ref: Option<BudgetRef>,
    scenario: Scenario,
    spins: Option<u32>,
    device: bool,
    /// The device `--device` records, when one is named.
    device_id: Option<String>,
    /// Where `--device` keeps its trace. None deletes it after the check.
    device_trace: Option<PathBuf>,
    /// Whether `--device` opens the page in the phone's Safari.
    device_open: bool,
    driver: DriverArg,
    /// Where `--driver serve` serves.
    serve: ServeOptions,
    /// The query string the served game loads with. It can hold a session, so no result
    /// records it.
    serve_query: Option<String>,
}

/// The game frame is idle after this long with no new requests.
const IDLE_AFTER_MS: u32 = 10_000;

/// A browser run stops after this long.
const RUN_TIMEOUT_MS: u32 = 120_000;

/// A device recording stops by itself after this long, if dyno does not stop it: the wait for a
/// browser, the run, and a minute more.
const DEVICE_TRACE_LIMIT: Duration =
    Duration::from_secs(DEFAULT_WAIT_FOR_BROWSER.as_secs() + RUN_TIMEOUT_MS as u64 / 1_000 + 60);

/// Runs `dyno check` and returns the status for the exit code.
pub fn run(args: CheckArgs) -> Result<Status, CliError> {
    let serve_flags =
        args.serve_host.is_some() || args.serve_port.is_some() || args.serve_query.is_some();
    if serve_flags && args.driver != DriverArg::Serve {
        return Err(CliError::usage(
            "--serve-host, --serve-port and --serve-query need --driver serve",
        ));
    }
    let options = CheckOptions {
        scenario: args.scenario.into(),
        spins: args.spins,
        device: args.device || args.device_id.is_some(),
        device_id: args.device_id,
        // A games file checks several games, so only one target keeps its trace.
        device_trace: match (&args.out, &args.games) {
            (Some(out), None) => Some(device::kept_trace(out)),
            _ => None,
        },
        device_open: !args.no_open,
        driver: args.driver,
        serve: ServeOptions {
            host: args.serve_host,
            port: args.serve_port.unwrap_or(DEFAULT_PORT),
            wait_for_browser: DEFAULT_WAIT_FOR_BROWSER,
        },
        serve_query: args.serve_query,
        ..CheckOptions::new(args.host_profile, args.budget.as_deref())?
    };
    let place = ResultPlace::of_out(args.out.as_deref());
    if let Some(games_file) = &args.games {
        let catalogue = check_catalogue(games_file, args.shard, &options)?;
        output::write_json(&catalogue, args.out.as_deref())?;
        output::print_summary(&output::catalogue_summary(&catalogue, place));
        return Ok(catalogue.status);
    }
    let target = parse_targets(&args.targets)?;
    let release = Release {
        game: args.game,
        version: args.release,
    };
    let result = check_target(&target, release, &options)?;
    output::write_json(&result, args.out.as_deref())?;
    output::print_summary(&output::check_summary(&result, place));
    Ok(result.deterministic.status)
}

/// Checks one target and returns its result. It writes nothing.
pub fn check_target(
    target: &CheckTarget,
    release: Release,
    options: &CheckOptions,
) -> Result<CheckResult, CliError> {
    check_run_flags(options, target)?;
    check_one(target, release, options)
}

impl CheckTarget {
    /// A target from a build and a launch URL. It needs at least one. A launch URL that does not
    /// parse is a usage error.
    pub fn new(build: Option<PathBuf>, launch_url: Option<&str>) -> Result<CheckTarget, CliError> {
        if build.is_none() && launch_url.is_none() {
            return Err(CliError::usage("give a build, a launch URL or both"));
        }
        let launch_url = match launch_url {
            Some(text) => Some(LaunchUrl::parse(text).map_err(CliError::usage)?),
            None => None,
        };
        Ok(CheckTarget { build, launch_url })
    }
}

impl CheckOptions {
    /// The options of a load-and-idle run in the built-in driver, with this host profile and
    /// budget file. A bad budget file is a usage error.
    pub fn new(
        host_profile: HostProfile,
        budget_path: Option<&Path>,
    ) -> Result<CheckOptions, CliError> {
        let budget = load_budget(budget_path)?;
        let budget_ref = match &budget {
            Some(budget) => Some(budget.reference().map_err(CliError::internal)?),
            None => None,
        };
        Ok(CheckOptions {
            host_profile,
            budget,
            budget_ref,
            scenario: Scenario::Alone,
            spins: None,
            device: false,
            device_id: None,
            device_trace: None,
            device_open: true,
            driver: DriverArg::Cdp,
            serve: ServeOptions {
                host: None,
                port: DEFAULT_PORT,
                wait_for_browser: DEFAULT_WAIT_FOR_BROWSER,
            },
            serve_query: None,
        })
    }
}

/// Reads the budget file, if one was given. A bad file is a usage error.
pub fn load_budget(path: Option<&Path>) -> Result<Option<Budget>, CliError> {
    match path {
        Some(path) => Budget::load(path).map(Some).map_err(CliError::usage),
        None => Ok(None),
    }
}

/// An error when a flag asks for a browser or device run and there is no launch URL, or when
/// `--driver serve` has no build folder to serve or a launch URL it would not use.
///
/// Every check calls it before any work starts, so a bad flag never leaves half a catalogue.
fn check_run_flags(options: &CheckOptions, target: &CheckTarget) -> Result<(), CliError> {
    if options.driver == DriverArg::Serve {
        if target.build.is_none() || target.launch_url.is_some() {
            return Err(CliError::usage(
                "--driver serve serves the build to a browser, so give a build folder and no launch URL",
            ));
        }
        return Ok(());
    }
    if options.device {
        return Err(CliError::usage(
            "--device needs --driver serve: the phone's Safari opens the page that dyno serves",
        ));
    }
    let has_launch_url = target.launch_url.is_some();
    let asks_for_a_run = options.spins.is_some()
        || options.scenario != Scenario::Alone
        || options.driver != DriverArg::Cdp;
    if asks_for_a_run && !has_launch_url {
        return Err(CliError::usage(
            "--spins, a --scenario other than alone and a --driver other than cdp need a launch URL",
        ));
    }
    Ok(())
}

fn check_catalogue(
    games_file: &Path,
    shard: Option<Shard>,
    options: &CheckOptions,
) -> Result<CatalogueResult, CliError> {
    let games = GamesFile::load(games_file).map_err(CliError::usage)?;
    // Builds in a games file are relative to the file.
    let root = games_file.parent().unwrap_or(Path::new(""));
    // Read every selected game's target and check the flags before any check starts.
    let mut planned = Vec::new();
    for game in games.select(shard) {
        let target = catalogue_target(root, game).map_err(in_game(game))?;
        check_run_flags(options, &target).map_err(in_game(game))?;
        planned.push((game, target));
    }
    let mut entries = Vec::new();
    for (game, target) in planned {
        let result = check_one(&target, catalogue_release(game), options).map_err(in_game(game))?;
        entries.push(CatalogueEntry {
            id: game.id.clone(),
            result,
        });
    }
    // A selection with no games checked nothing, so it must not pass.
    let status = if entries.is_empty() {
        Status::Incomplete
    } else {
        worst(
            entries
                .iter()
                .map(|entry| entry.result.deterministic.status),
        )
    };
    Ok(CatalogueResult {
        schema_version: SCHEMA_VERSION,
        shard,
        status,
        games: entries,
    })
}

/// Puts the game's id before an error about it.
fn in_game(game: &GameEntry) -> impl Fn(CliError) -> CliError + '_ {
    move |error| error.with_context(&format!("game {:?}", game.id))
}

fn catalogue_target(root: &Path, game: &GameEntry) -> Result<CheckTarget, CliError> {
    let launch_url = match &game.launch_url {
        Some(text) => Some(LaunchUrl::parse(text).map_err(CliError::usage)?),
        None => None,
    };
    Ok(CheckTarget {
        build: game.build.as_ref().map(|build| root.join(build)),
        launch_url,
    })
}

fn catalogue_release(game: &GameEntry) -> Release {
    Release {
        game: Some(game.id.clone()),
        version: game.release.clone(),
    }
}

/// Splits the positional targets into at most one build and at most one launch URL.
fn parse_targets(values: &[String]) -> Result<CheckTarget, CliError> {
    let mut target = CheckTarget::default();
    for value in values {
        let duplicate = if is_launch_url(value) {
            let launch_url = LaunchUrl::parse(value).map_err(CliError::usage)?;
            target.launch_url.replace(launch_url).is_some()
        } else {
            target.build.replace(PathBuf::from(value)).is_some()
        };
        if duplicate {
            return Err(CliError::usage(
                "give at most one build folder or .zip and at most one launch URL",
            ));
        }
    }
    Ok(target)
}

fn is_launch_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn check_one(
    target: &CheckTarget,
    release: Release,
    options: &CheckOptions,
) -> Result<CheckResult, CliError> {
    let clock = Clock::start();
    // A served run's launch URL is dyno's own server. The result records the build, not that URL.
    let mut served = match (&options.driver, &target.build) {
        (DriverArg::Serve, Some(build)) => Some(serve(build, &options.serve)?),
        _ => None,
    };
    let launch_url = match &served {
        Some(driver) => Some(
            LaunchUrl::parse(&driver.launch_url(options.serve_query.as_deref()))
                .map_err(CliError::usage)?,
        ),
        None => target.launch_url.clone(),
    };
    let has_launch_url = launch_url.is_some();

    let scan = match &target.build {
        Some(build) => dyno_static::scan(build, &options.host_profile, options.budget.as_ref())
            .map_err(CliError::usage)?,
        None => dyno_static::without_build(),
    };
    let verdicts = match &options.budget {
        Some(budget) => budget_verdicts(budget, &scan.measures),
        None => Vec::new(),
    };

    let mut run_measures = model_measures(has_launch_url);
    let mut run_findings = Vec::new();
    let mut driver = None;
    // The recording starts before the person is told to open the page, so it sees the whole run.
    // Safari opens the page before the recording starts, as a launch ends a recording that is on.
    // The page waits for the run to start.
    let device_run = match (&served, options.device) {
        (Some(served), true) => Some(device::start(
            options.device_id.as_deref(),
            options.device_open.then(|| served.harness_url()),
            options.device_trace.as_deref(),
            DEVICE_TRACE_LIMIT,
        )?),
        _ => None,
    };
    if let Some(device_run) = &device_run {
        output::print_summary(&device_run.lines());
    }
    if let Some(served) = &served {
        output::print_summary(&output::serve_lines(
            served.harness_url(),
            options.serve.wait_for_browser,
        ));
    }
    if let Some(launch_url) = &launch_url {
        let request = RunRequest {
            protocol_version: RUNNER_PROTOCOL_VERSION,
            launch_url: launch_url.as_str().to_owned(),
            host_profile: options.host_profile.clone(),
            scenario: options.scenario,
            spins: options.spins,
            idle_after_ms: IDLE_AFTER_MS,
            timeout_ms: RUN_TIMEOUT_MS,
        };
        let lab = match served.as_mut() {
            Some(served) => dyno_run::run_lab(served, &request),
            None => dyno_run::run_lab(new_driver(&options.driver).as_mut(), &request),
        };
        run_measures.extend(lab.measures);
        run_findings = lab.findings;
        driver = lab.driver;
    }
    if let Some(device_run) = device_run {
        output::print_summary(&[device_run.saving_line()]);
        run_measures.extend(device_run.finish(served.as_ref().and_then(ServeDriver::last_run)));
    }

    let inputs = Inputs {
        build: target.build.is_some(),
        launch_url: has_launch_url,
        host_profile: options.host_profile.reference(),
        budget: options.budget_ref.clone(),
        scenario: options.scenario,
        spins: options.spins,
        device: options.device,
    };
    let run_coverage = run_coverage(&inputs, &run_measures);
    let status = overall_status(
        &inputs,
        &scan.measures,
        &verdicts,
        &scan.findings,
        run_coverage,
    );
    let deterministic = Deterministic {
        versions: Versions {
            schema: SCHEMA_VERSION,
            tool: TOOL_VERSION.to_owned(),
            scoring: SCORING_VERSION,
            model: MODEL_VERSION.map(str::to_owned),
        },
        inputs,
        build: scan.manifest,
        status,
        run_coverage,
        measures: scan.measures,
        verdicts,
        findings: scan.findings,
    };
    let run = RunInfo {
        started_at: clock.started_at(),
        duration_ms: clock.elapsed_ms(),
        machine: machine(),
        tool_build: tool_build(),
        target: Target {
            build: target
                .build
                .as_ref()
                .map(|build| build.display().to_string()),
            launch_url: target.launch_url.as_ref().map(LaunchUrl::recorded),
        },
        release,
        driver,
        measures: run_measures,
        findings: run_findings,
        tier: provisional_tier(has_launch_url),
    };
    CheckResult::new(deterministic, run).map_err(CliError::internal)
}

fn new_driver(driver: &DriverArg) -> Box<dyn Driver> {
    match driver {
        // A served run gets its driver from `serve`, bound to its build, so this is not reached.
        DriverArg::Cdp | DriverArg::Serve => Box::new(CdpDriver),
        DriverArg::Exec(program) => Box::new(ExecDriver::new(program)),
    }
}

/// Binds the served run's servers. A build that is not a folder is a usage error; a port in use
/// is an I/O error.
fn serve(build: &Path, options: &ServeOptions) -> Result<ServeDriver, CliError> {
    ServeDriver::bind(build, options).map_err(|error| match error {
        ServeError::Build(_) => CliError::usage(error),
        _ => CliError::internal(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(values: &[&str]) -> Result<CheckTarget, CliError> {
        let values: Vec<String> = values.iter().map(|value| (*value).to_owned()).collect();
        parse_targets(&values)
    }

    #[test]
    fn a_build_and_a_launch_url_fill_one_target() {
        let both = targets(&["https://example.com/game?session=1", "dist"]).unwrap();
        assert_eq!(both.build, Some(PathBuf::from("dist")));
        assert_eq!(
            both.launch_url.as_ref().map(LaunchUrl::as_str),
            Some("https://example.com/game?session=1")
        );
    }

    #[test]
    fn a_folder_whose_name_starts_with_http_is_a_build() {
        let build = targets(&["HTTP-folder"]).unwrap();
        assert_eq!(build.build, Some(PathBuf::from("HTTP-folder")));
        assert_eq!(build.launch_url, None);
    }

    #[test]
    fn the_url_scheme_is_matched_in_any_case() {
        let url = targets(&["HTTP://example.com"]).unwrap();
        assert_eq!(
            url.launch_url.as_ref().map(LaunchUrl::as_str),
            Some("http://example.com/")
        );
    }

    #[test]
    fn a_launch_url_that_does_not_parse_is_a_usage_error() {
        assert!(matches!(
            targets(&["https://u:abc/def@h/g"]),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn a_target_needs_a_build_or_a_launch_url() {
        assert!(matches!(
            CheckTarget::new(None, None),
            Err(CliError::Usage(_))
        ));
        assert!(CheckTarget::new(Some(PathBuf::from("dist")), None).is_ok());
        assert!(CheckTarget::new(None, Some("https://example.com/game")).is_ok());
    }

    #[test]
    fn a_target_launch_url_must_be_http() {
        let target = CheckTarget::new(None, Some("file:///game/index.html"));
        assert!(matches!(target, Err(CliError::Usage(_))));
    }

    #[test]
    fn two_builds_or_two_urls_are_a_usage_error() {
        assert!(matches!(targets(&["a", "b"]), Err(CliError::Usage(_))));
        assert!(matches!(
            targets(&["https://a.example", "http://b.example"]),
            Err(CliError::Usage(_))
        ));
    }
}
