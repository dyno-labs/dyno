//! The command line. `--help` marks each command and flag with the plan phase that builds it.

use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Parser, Subcommand, ValueEnum};
use dyno_schema::catalogue::Shard;
use dyno_schema::profile::{DEFAULT_HOST_PROFILE, HostProfile, Scenario};

#[derive(Debug, Parser)]
#[command(
    name = "dyno",
    bin_name = "dyno",
    version,
    about = "Predicts how much memory an HTML5 game in an iframe will use on an iPhone, from its build.",
    long_about = "Predicts how much memory an HTML5 game in an iframe will use on an iPhone, from its \
build.\n\ncheck, compare and doctor each write one canonical JSON result to stdout, or to --out. \
A short summary for people goes to stderr. It has no stable format, so do not parse it. login, \
logout and upload write only that summary.",
    after_help = EXIT_CODES,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// [phase 1] Check a build folder, a launch URL, or every game in a games file
    #[command(after_help = EXIT_CODES)]
    Check(Box<CheckArgs>),
    /// [phase 1] Diff two result files from `dyno check`
    #[command(after_help = EXIT_CODES)]
    Compare(CompareArgs),
    /// [phase 2] Check that this Mac can run the game on an iPhone
    #[command(after_help = EXIT_CODES)]
    Doctor(DoctorArgs),
    /// [phase 3] Serve check, compare and read_result to an MCP client on stdin and stdout
    #[command(after_help = MCP_EXIT_CODES)]
    Mcp,
    /// [phase 3] Sign in to the dyno site with a token from its Account page, to upload results
    #[command(after_help = LOGIN_EXIT_CODES)]
    Login(LoginArgs),
    /// [phase 3] Remove the token that dyno login saved
    #[command(after_help = LOGOUT_EXIT_CODES)]
    Logout,
    /// [phase 3] Upload result files from `dyno check` to the dyno site, and print each one's link
    #[command(after_help = UPLOAD_EXIT_CODES)]
    Upload(UploadArgs),
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// [phase 1] A build folder or .zip (static scan), an http(s) launch URL (browser run), or
    /// one of each
    #[arg(
        value_name = "TARGET",
        num_args = 1..=MAX_TARGETS,
        required_unless_present_any = ["games", "baseline"],
        conflicts_with_all = ["games", "baseline"]
    )]
    pub targets: Vec<String>,

    /// [phase 2] Check the empty build inside dyno instead of a target: one page with nothing on
    /// it, whose build id never changes. With --device, it gives the phone's footprint with no
    /// game
    #[arg(long, conflicts_with_all = ["games", "game", "release", "game_id"])]
    pub baseline: bool,

    /// [phase 1] Check every game in a games file (TOML, or JSON by the .json extension)
    #[arg(long, value_name = "FILE")]
    pub games: Option<PathBuf>,

    /// [phase 1] Check only shard I of N of the games file, such as 1/4
    // clap skips `requires` when the required flag conflicts with one that is present, so the
    // conflict with TARGET is spelled out.
    #[arg(
        long,
        value_name = "I/N",
        requires = "games",
        conflicts_with = "targets"
    )]
    pub shard: Option<Shard>,

    /// [phase 1] The budget file (TOML, or JSON by the .json extension)
    #[arg(long, value_name = "FILE")]
    pub budget: Option<PathBuf>,

    /// [phase 1] Write the JSON result to FILE instead of stdout
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// [phase 1] How the host page embeds the game, and the contract it checks
    #[arg(
        long,
        value_name = "NAME",
        default_value = DEFAULT_HOST_PROFILE,
        value_parser = host_profile_parser()
    )]
    pub host_profile: HostProfile,

    /// [phase 1] The game name to label the result with. Outside the result hash
    #[arg(long, value_name = "NAME", conflicts_with = "games")]
    pub game: Option<String>,

    /// [phase 1] The release version to label the result with, such as v12. Outside the result
    /// hash
    #[arg(long, value_name = "VERSION", conflicts_with = "games")]
    pub release: Option<String>,

    /// [phase 3] The game's id on the dyno site, a UUID. Default: the entry for the build in the
    /// nearest dyno.toml above it. Outside the result hash
    #[arg(
        long,
        value_name = "UUID",
        conflicts_with = "games",
        value_parser = parse_game_id
    )]
    pub game_id: Option<String>,

    /// [phase 1] The browser driver: cdp, the built-in Chromium driver; exec:PROGRAM, a program
    /// that speaks the runner protocol on stdin and stdout; or serve, which serves the build and
    /// waits for a browser on the network to open it, such as Safari on an iPhone. exec needs a
    /// launch URL, and serve needs a build folder and no launch URL
    #[arg(long, value_name = "DRIVER", default_value = "cdp")]
    pub driver: DriverArg,

    /// [phase 1] With --driver serve, the address a browser reaches this machine at. Default:
    /// this machine's address on its local network
    #[arg(long, value_name = "HOST")]
    pub serve_host: Option<String>,

    /// [phase 1] With --driver serve, the harness page's port. The game is on the next one
    /// [default: 7370]
    #[arg(long, value_name = "PORT")]
    pub serve_port: Option<u16>,

    /// [phase 1] With --driver serve, the query string the game loads with, such as a demo
    /// session's sessionID and rgs_url. No result records it
    #[arg(long, value_name = "QUERY")]
    pub serve_query: Option<String>,

    /// [phase 2] The harness scenario of the browser run. Any scenario but `alone` needs a
    /// launch URL
    #[arg(long, value_enum, default_value_t = ScenarioArg::Alone)]
    pub scenario: ScenarioArg,

    /// [phase 2] Play N demo spins after load. Needs a launch URL
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub spins: Option<u32>,

    /// [phase 2] Also record the game's memory on the connected iPhone with xctrace, while its
    /// Safari runs the check. Needs --driver serve and a Mac with Xcode
    #[arg(long)]
    pub device: bool,

    /// [phase 2] With --device, the device to record, by its UDID or name as `xcrun xctrace list
    /// devices` gives them. Default: the only connected iPhone
    #[arg(long, value_name = "ID")]
    pub device_id: Option<String>,

    /// [phase 2] With --device, do not open the page in the phone's Safari: open it by hand
    #[arg(long)]
    pub no_open: bool,

    /// [phase 2] With --device, run the empty build and the game N times each, in turn, each in a
    /// fresh Safari with its own recording. The device measures are then the medians of the runs,
    /// the kills in N runs, and the game's peak over the empty build's
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..),
        conflicts_with = "baseline"
    )]
    pub runs: Option<u32>,

    /// [phase 3] Upload the result to the dyno site and print its link, or each game's with
    /// --games. Needs dyno login or DYNO_TOKEN
    #[arg(long)]
    pub upload: bool,
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// [phase 3] The token, instead of pasting it on stdin. It shows in the shell's history
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
}

#[derive(Debug, Args)]
pub struct UploadArgs {
    /// [phase 3] Result files from `dyno check`
    #[arg(value_name = "RESULT", required = true)]
    pub results: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CompareArgs {
    /// [phase 1] The base result file from `dyno check`, such as the last release
    #[arg(value_name = "BASE")]
    pub base: PathBuf,

    /// [phase 1] The head result file from `dyno check`, such as this build
    #[arg(value_name = "HEAD")]
    pub head: PathBuf,

    /// [phase 1] Apply this budget file to both sides, instead of each side's own verdicts
    #[arg(long, value_name = "FILE")]
    pub budget: Option<PathBuf>,

    /// [phase 1] Write the JSON result to FILE instead of stdout
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// [phase 2] Write the JSON report to FILE instead of stdout
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

/// The harness scenarios, as `--scenario` spells them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ScenarioArg {
    /// [phase 1] The game alone in a fresh harness page
    Alone,
    /// [phase 2] The harness page after 10 earlier games
    Aged,
    /// [phase 2] The game active while the next one preloads
    Overlap,
}

/// The browser drivers, as `--driver` spells them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverArg {
    /// The Rust CDP driver, built in: `cdp`.
    Cdp,
    /// A program that speaks the runner protocol: `exec:PROGRAM`.
    Exec(PathBuf),
    /// dyno serves the build to a browser on the network: `serve`.
    Serve,
}

/// What `--driver` puts before a program.
const EXEC_PREFIX: &str = "exec:";

impl FromStr for DriverArg {
    type Err = String;

    fn from_str(text: &str) -> Result<DriverArg, String> {
        match text {
            "cdp" => return Ok(DriverArg::Cdp),
            "serve" => return Ok(DriverArg::Serve),
            _ => {}
        }
        match text.strip_prefix(EXEC_PREFIX) {
            Some(program) if !program.is_empty() => Ok(DriverArg::Exec(PathBuf::from(program))),
            _ => Err(format!(
                "driver {text:?} must be cdp, exec:PROGRAM or serve"
            )),
        }
    }
}

/// A check takes at most one build and one launch URL.
const MAX_TARGETS: usize = 2;

/// The exit codes, shown after each command's help.
const EXIT_CODES: &str = "\
Exit codes:
  0   pass, or warn
  1   fail: a budget or rule fails
  2   incomplete: a required measure is missing, or a budget or rule could not be checked
  3   invalid: a required measure or an input result failed its own check
  64  usage or config error
  70  internal or I/O error";

/// The exit codes of `dyno mcp`, which gives no result.
const MCP_EXIT_CODES: &str = "\
Exit codes:
  0   the client closed stdin
  64  usage error
  70  the MCP session did not start, or failed";

/// The exit codes of `dyno login`.
const LOGIN_EXIT_CODES: &str = "\
Exit codes:
  0   signed in, and the token is saved
  64  usage error, or the site does not take the token
  70  the site cannot be reached, or the token cannot be saved";

/// The exit codes of `dyno logout`.
const LOGOUT_EXIT_CODES: &str = "\
Exit codes:
  0   no token is saved now
  64  usage error
  70  the saved token cannot be removed";

/// The exit codes of `dyno upload`.
const UPLOAD_EXIT_CODES: &str = "\
Exit codes:
  0   every result uploaded
  64  usage error: not signed in, or a result file that cannot be read or is not JSON
  70  a result did not upload: the site cannot be reached, or did not take it";

/// Parses `--game-id`: a UUID, written in lower case with dashes.
fn parse_game_id(text: &str) -> Result<String, String> {
    dyno_schema::link::game_id(text)
}

/// Parses `--host-profile`. Help lists the known names, and the value is the profile itself.
fn host_profile_parser() -> impl TypedValueParser<Value = HostProfile> {
    PossibleValuesParser::new(HostProfile::NAMES).try_map(|name| {
        HostProfile::named(&name).ok_or_else(|| format!("unknown host profile {name:?}"))
    })
}

impl From<ScenarioArg> for Scenario {
    fn from(scenario: ScenarioArg) -> Scenario {
        match scenario {
            ScenarioArg::Alone => Scenario::Alone,
            ScenarioArg::Aged => Scenario::Aged,
            ScenarioArg::Overlap => Scenario::Overlap,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn assert_names_a_phase(help: Option<&clap::builder::StyledStr>, what: &str) {
        let help = help.map(ToString::to_string).unwrap_or_default();
        assert!(
            help.starts_with("[phase "),
            "{what} does not name its phase: {help:?}"
        );
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_command_and_flag_names_its_phase() {
        let cli = Cli::command();
        for command in cli.get_subcommands() {
            let name = command.get_name();
            assert_names_a_phase(command.get_about(), name);
            let own_arguments = command
                .get_arguments()
                .filter(|argument| argument.get_id() != "help");
            for argument in own_arguments {
                let what = format!("{name} {}", argument.get_id());
                assert_names_a_phase(argument.get_help(), &what);
            }
        }
    }

    #[test]
    fn driver_cdp_is_the_built_in_driver() {
        assert_eq!("cdp".parse(), Ok(DriverArg::Cdp));
    }

    #[test]
    fn driver_exec_names_a_program() {
        assert_eq!(
            "exec:./drivers/playwright.mjs".parse(),
            Ok(DriverArg::Exec(PathBuf::from("./drivers/playwright.mjs")))
        );
    }

    #[test]
    fn driver_serve_serves_the_build() {
        assert_eq!("serve".parse(), Ok(DriverArg::Serve));
    }

    #[test]
    fn a_driver_that_is_not_cdp_exec_with_a_program_or_serve_is_an_error() {
        for text in ["exec:", "playwright", "CDP", "serve:"] {
            assert!(text.parse::<DriverArg>().is_err(), "{text}");
        }
    }

    #[test]
    fn every_scenario_names_its_phase() {
        for scenario in ScenarioArg::value_variants() {
            let value = scenario.to_possible_value().unwrap();
            assert_names_a_phase(value.get_help(), value.get_name());
        }
    }
}
