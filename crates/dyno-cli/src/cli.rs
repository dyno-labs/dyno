//! The command line. `--help` marks each command and flag with the plan phase that builds it.

use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use dyno_schema::catalogue::Shard;
use dyno_schema::profile::{DEFAULT_HOST_PROFILE, HostProfile, Scenario};

#[derive(Debug, Parser)]
#[command(
    name = "dyno",
    bin_name = "dyno",
    version,
    about = "Predicts how much memory an HTML5 game in an iframe will use on an iPhone, from its build.",
    long_about = "Predicts how much memory an HTML5 game in an iframe will use on an iPhone, from its \
build.\n\ncheck, compare, doctor, verify and receipt each write one canonical JSON result to \
stdout, or to --out. A short summary for people goes to stderr. It has no stable format, so do not \
parse it. login, logout, upload and link write only that summary.",
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
    /// [phase 3] Link a build folder to its game on the dyno site, in dyno.toml
    #[command(after_help = LINK_EXIT_CODES)]
    Link(LinkArgs),
    /// [phase 3] Check a lab's signed receipt for a result, against the lab's public key
    #[command(after_help = VERIFY_EXIT_CODES)]
    Verify(VerifyArgs),
    /// [phase 3] Run a lab: make its key pair, and sign a receipt for a result
    #[command(subcommand, after_help = RECEIPT_EXIT_CODES)]
    Receipt(ReceiptCommand),
}

/// What `dyno receipt` does.
#[derive(Debug, Subcommand)]
pub enum ReceiptCommand {
    /// [phase 3] Make a lab's key pair: a private key file, and the public keys document
    #[command(after_help = KEYGEN_EXIT_CODES)]
    Keygen(KeygenArgs),
    /// [phase 3] Sign a receipt for a result file from `dyno check` with a run on a device
    #[command(after_help = SIGN_EXIT_CODES)]
    Sign(SignArgs),
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

    /// [phase 1] Serve the build folder on 127.0.0.1 and load it in dyno's own Chromium, so the
    /// browser loads the files that the scan read. Needs a build folder, no launch URL and the
    /// cdp driver
    #[arg(long)]
    pub browser: bool,

    /// [phase 1] The browser driver: cdp, the built-in Chromium driver; exec:PROGRAM, a program
    /// that speaks the runner protocol on stdin and stdout; or serve, which serves the build and
    /// waits for a browser on the network to open it, such as Safari on an iPhone. exec needs a
    /// launch URL, and serve needs a build folder and no launch URL
    #[arg(long, value_name = "DRIVER", default_value = "cdp")]
    pub driver: DriverArg,

    /// [phase 1] With --driver serve, the address a browser reaches this machine at. Default:
    /// with --device on a Mac, this machine's address in the device link's tunnel to the phone,
    /// when it has one; else this machine's address on its local network
    #[arg(long, value_name = "HOST")]
    pub serve_host: Option<String>,

    /// [phase 1] With --driver serve, the harness page's port. Each run's game takes a port of its
    /// own from the 64 after it, so open PORT to PORT + 64 on a firewall [default: 7370]
    #[arg(long, value_name = "PORT")]
    pub serve_port: Option<u16>,

    /// [phase 1] With --driver serve, the query string the game loads with, such as a demo
    /// session's sessionID and rgs_url. No result records it
    #[arg(long, value_name = "QUERY")]
    pub serve_query: Option<String>,

    /// [phase 1] With --driver serve, start this program with the harness page's URL as its only
    /// argument, read the memory of its processes through the run (Linux), and stop it when the
    /// run ends. A script can set the browser up first, such as examples/webkit/open.sh
    #[arg(
        long,
        value_name = "PROGRAM",
        conflicts_with_all = ["device", "device_id", "device_ip"]
    )]
    pub open_with: Option<PathBuf>,

    /// [phase 2] The harness scenario of the browser run. Any scenario but `alone` needs a
    /// launch URL
    #[arg(long, value_enum, default_value_t = ScenarioArg::Alone)]
    pub scenario: ScenarioArg,

    /// [phase 2] Play N demo spins after load. Needs a launch URL
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub spins: Option<u32>,

    /// [phase 2] Also record the game's memory on an iPhone, on a cable or on the network, while
    /// its Safari runs the check. Needs --driver serve, and iOS 17 or later with Developer Mode on
    #[arg(long)]
    pub device: bool,

    /// [phase 2] With --device, the iPhone to record, by its UDID, of those that usbmuxd lists.
    /// Default: the only one
    #[arg(long, value_name = "UDID", conflicts_with = "device_ip")]
    pub device_id: Option<String>,

    /// [phase 2] Record the iPhone at this address on the network, with no usbmuxd: --device with
    /// no cable, from Linux too. Needs --pairing-file
    #[arg(long, value_name = "IP", requires = "pairing_file")]
    pub device_ip: Option<IpAddr>,

    /// [phase 2] With --device-ip, the phone's pair record: a plist from a computer the phone
    /// trusts, such as /var/db/lockdown/<UDID>.plist on a Mac
    #[arg(long, value_name = "FILE", requires = "device_ip")]
    pub pairing_file: Option<PathBuf>,

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

    /// [phase 2] The kind of lab that runs the check, such as desk, rig or devicefarm: a short
    /// word in lower case, of the letters a to z, digits and -. Outside the result hash
    #[arg(long, value_name = "NAME", value_parser = parse_lab_name)]
    pub lab: Option<String>,

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
#[command(group(ArgGroup::new("game_to_link").required(true).args(["name", "game_id"])))]
pub struct LinkArgs {
    /// [phase 3] The build folder or .zip that dyno check is given
    #[arg(value_name = "BUILD")]
    pub build: PathBuf,

    /// [phase 3] The game's name on the dyno site. dyno makes the game when the site has none of
    /// that name
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// [phase 3] The game's id on the dyno site, a UUID
    #[arg(long, value_name = "UUID", value_parser = parse_game_id)]
    pub game_id: Option<String>,

    /// [phase 3] The dyno.toml to write, in a folder above the build [default: dyno.toml]
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// [phase 3] The receipt file that the lab gave with the result
    #[arg(value_name = "RECEIPT")]
    pub receipt: PathBuf,

    /// [phase 3] A public key of the lab, which you got from the lab and trust: its raw 32 bytes
    /// in base64url. Give the flag again for each more key
    #[arg(long, value_name = "KEY")]
    pub key: Vec<String>,

    /// [phase 3] The lab's public keys document, which says when each key was retired: a file, or
    /// an https URL
    #[arg(long, value_name = "FILE|URL")]
    pub keys: Option<String>,

    /// [phase 3] Also check that this result file is the one the receipt is for
    #[arg(long, value_name = "FILE")]
    pub result: Option<PathBuf>,

    /// [phase 3] Also check that this build folder or .zip has the receipt's build id, so the
    /// receipt is for the files you hold
    #[arg(long, value_name = "BUILD")]
    pub build: Option<PathBuf>,

    /// [phase 3] Write the JSON verdict to FILE instead of stdout
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct KeygenArgs {
    /// [phase 3] Write the private key to FILE, which must not be there yet. Keep it secret
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,

    /// [phase 3] Write the public keys document to FILE, which must not be there yet, instead of
    /// stdout
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct SignArgs {
    /// [phase 3] The private key file that `dyno receipt keygen` wrote
    #[arg(long, value_name = "FILE")]
    pub key: PathBuf,

    /// [phase 3] The result file from `dyno check` to sign a receipt for
    #[arg(long, value_name = "FILE")]
    pub result: PathBuf,

    /// [phase 3] The lab's name, such as its domain
    #[arg(long, value_name = "NAME")]
    pub issuer: String,

    /// [phase 3] The lab's id for the run
    #[arg(long, value_name = "ID")]
    pub check_id: String,

    /// [phase 3] What kind of lab it is, in the lab's own words
    #[arg(long, value_name = "KIND", default_value = DEFAULT_LAB_KIND)]
    pub lab_kind: String,

    /// [phase 3] What the lab's devices run on, such as the name of a device cloud
    #[arg(long, value_name = "NAME")]
    pub lab_backend: Option<String>,

    /// [phase 3] When the lab signs, in UTC to the second, such as 2026-11-25T10:00:00Z
    /// [default: now]
    #[arg(long, value_name = "TIME", value_parser = parse_receipt_time)]
    pub issued_at: Option<String>,

    /// [phase 3] Write the receipt to FILE, which must not be there yet, instead of stdout
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

/// The lab kind of `dyno receipt sign` when `--lab-kind` does not give one: a lab that runs the
/// phones it owns.
pub const DEFAULT_LAB_KIND: &str = "own_phones";

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

/// Parses `--lab`: a short word in lower case.
fn parse_lab_name(text: &str) -> Result<String, String> {
    dyno_schema::lab::lab_name(text)
}

/// The exit codes of `dyno link`.
const LINK_EXIT_CODES: &str = "\
Exit codes:
  0   linked: dyno.toml names the build's game
  64  usage error: not signed in, a build or dyno.toml that cannot be read, a build that is not
      below dyno.toml's folder or has a nearer dyno.toml, or a game or token the site does not
      know
  70  the site cannot be reached, or dyno.toml cannot be written";

/// The exit codes of `dyno verify`.
const VERIFY_EXIT_CODES: &str = "\
Exit codes:
  0   verified: a key you gave signed the receipt, and everything you asked to check matches it
  1   not verified: no key you gave signed it, the key was retired, or the result or the build
      is not the one the receipt is for
  64  usage or input error: no key given, or a receipt, key, keys document, result or build that
      cannot be read or is not one
  70  the keys document's URL cannot be reached, or the verdict cannot be written";

/// The exit codes of `dyno receipt`, for both of its commands.
const RECEIPT_EXIT_CODES: &str = "\
Exit codes:
  0   the key pair is made, or the receipt is signed
  64  usage or input error: a private key file that is there already or is not one, or a result
      that cannot be read, was edited, or has no build or no device
  70  internal or I/O error, such as a file that cannot be written";

/// The exit codes of `dyno receipt keygen`.
const KEYGEN_EXIT_CODES: &str = "\
Exit codes:
  0   the key pair is made
  64  usage error, or a private key file that is there already
  70  internal or I/O error, such as a file that cannot be written";

/// The exit codes of `dyno receipt sign`.
const SIGN_EXIT_CODES: &str = "\
Exit codes:
  0   the receipt is signed
  64  usage or input error: a private key file that cannot be read or is not one, or a result
      that cannot be read, was edited, or has no build or no device
  70  internal or I/O error, such as a receipt file that cannot be written";

/// Parses `--issued-at`: a time as a receipt states it.
fn parse_receipt_time(text: &str) -> Result<String, String> {
    if dyno_receipt::is_receipt_time(text) {
        Ok(text.to_owned())
    } else {
        Err("it must be in UTC, to the second, such as 2026-11-25T10:00:00Z".to_owned())
    }
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

    /// Checks a command, its flags, and the commands under it, such as `receipt sign`.
    fn assert_command_names_its_phases(command: &clap::Command) {
        let name = command.get_name();
        assert_names_a_phase(command.get_about(), name);
        let own_arguments = command
            .get_arguments()
            .filter(|argument| argument.get_id() != "help");
        for argument in own_arguments {
            let what = format!("{name} {}", argument.get_id());
            assert_names_a_phase(argument.get_help(), &what);
        }
        for under in command.get_subcommands() {
            assert_command_names_its_phases(under);
        }
    }

    #[test]
    fn every_command_and_flag_names_its_phase() {
        let cli = Cli::command();
        for command in cli.get_subcommands() {
            assert_command_names_its_phases(command);
        }
    }

    #[test]
    fn issued_at_is_a_time_as_a_receipt_states_it() {
        assert_eq!(
            parse_receipt_time("2026-11-25T10:00:00Z"),
            Ok("2026-11-25T10:00:00Z".to_owned())
        );
        for text in [
            "2026-11-25T10:00:00.000Z",
            "2026-11-25T11:00:00+01:00",
            "now",
        ] {
            assert!(parse_receipt_time(text).is_err(), "{text}");
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

    /// The `--lab` of a check of `dist` with these extra arguments.
    fn lab_of(extra: &[&str]) -> Result<Option<String>, clap::Error> {
        let arguments = ["dyno", "check", "dist"].iter().chain(extra);
        Cli::try_parse_from(arguments).map(|cli| match cli.command {
            Command::Check(check) => check.lab,
            _ => None,
        })
    }

    #[test]
    fn a_check_names_no_lab_unless_asked() {
        assert_eq!(lab_of(&[]).unwrap(), None);
        assert_eq!(
            lab_of(&["--lab", "devicefarm"]).unwrap().as_deref(),
            Some("devicefarm")
        );
    }

    #[test]
    fn a_lab_name_that_is_not_a_short_lower_case_word_is_a_usage_error() {
        for name in ["Desk", "phone lab", "", "2rig", &"a".repeat(33)] {
            let error = lab_of(&["--lab", name]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
            assert!(error.to_string().contains("lower case"), "{error}");
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
