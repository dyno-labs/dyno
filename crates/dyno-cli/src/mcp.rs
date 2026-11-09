//! `dyno mcp`: a local MCP server on stdin and stdout (Phase 3), so that an AI coding
//! tool can run dyno. There is no AI inside dyno.
//!
//! It serves three tools: `check`, `compare` and `read_result`. Each one calls the same functions
//! as its command and gives two text blocks: canonical JSON, then the summary for people. A check
//! that fails or warns is still a successful call, because the status is in the result. A usage or
//! internal error is a tool error with the message. Only protocol messages go to stdout.

use std::path::PathBuf;
use std::thread;

use dyno_schema::TOOL_VERSION;
use dyno_schema::canonical::to_canonical_json;
use dyno_schema::profile::{DEFAULT_HOST_PROFILE, HostProfile};
use dyno_schema::result::Release;
use dyno_schema::status::Status;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::check::{CheckOptions, CheckTarget, check_target};
use crate::compare::{compare_files, read_result};
use crate::exit::CliError;
use crate::output::{self, ResultPlace};

/// The name the server gives in `initialize`.
const SERVER_NAME: &str = "dyno";

/// What the server tells a client in `initialize`.
const INSTRUCTIONS: &str = "dyno predicts how much memory an HTML5 game in an iframe will use on \
an iPhone, from its build. Call check on a build folder or .zip, a launch URL, or both. Give check \
an out path to keep the result, then call compare on two such files. Each tool gives canonical \
JSON, then a short summary for people. Relative paths are relative to the folder the server runs \
in.";

const CHECK_DESCRIPTION: &str = "Check a game build folder or .zip, an http(s) launch URL, or \
both, as `dyno check` does. Gives the canonical JSON result, then a summary. The status is in the \
result: a check that fails is not a tool error.";

const COMPARE_DESCRIPTION: &str = "Diff two result files from `dyno check`, as `dyno compare` \
does. Gives the canonical JSON compare result, then a summary.";

const READ_RESULT_DESCRIPTION: &str = "Read a result file from `dyno check`. Gives its status, its \
result_hash and whether the hash matches the content as JSON, then the check summary.";

/// The first summary line for a result whose hash does not match its content.
const EDITED: &str = "this result was edited: its result_hash does not match its content, so do \
not trust its status";

/// What `dyno mcp` writes on stderr when it starts.
const SERVING: &str = "dyno mcp: serving check, compare and read_result on stdin and stdout";

/// What the `check` tool takes.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckParams {
    /// A build folder or .zip for the static scan.
    build: Option<PathBuf>,
    /// An http(s) launch URL for a browser run.
    launch_url: Option<String>,
    /// A budget file: TOML, or JSON by the .json extension.
    budget: Option<PathBuf>,
    /// How the host page embeds the game, and the contract it checks. Default "reels".
    host_profile: Option<String>,
    /// The game name to label the result with. Outside the result hash.
    game: Option<String>,
    /// The release version to label the result with, such as v12. Outside the result hash.
    release: Option<String>,
    /// Also write the JSON result to this file, for a later compare.
    out: Option<PathBuf>,
}

/// What the `compare` tool takes.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareParams {
    /// The base result file from `dyno check`, such as the last release.
    base: PathBuf,
    /// The head result file from `dyno check`, such as this build.
    head: PathBuf,
    /// Apply this budget file to both sides, instead of each side's own verdicts.
    budget: Option<PathBuf>,
}

/// What the `read_result` tool takes.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadResultParams {
    /// A result file from `dyno check`.
    path: PathBuf,
}

/// What the `read_result` tool says about a result file.
#[derive(Debug, Serialize)]
struct ResultStatus {
    status: Status,
    result_hash: String,
    /// Whether `result_hash` matches the `deterministic` JSON as the file holds it.
    hash_matches: bool,
}

/// What a tool gives back when it works.
struct ToolAnswer {
    /// Canonical JSON.
    json: String,
    /// The summary for people.
    summary: Vec<String>,
}

/// The MCP server and its tools.
#[derive(Clone, Copy, Debug)]
pub struct DynoServer;

/// Serves MCP on stdin and stdout until the client closes stdin.
pub fn run() -> Result<(), CliError> {
    eprintln!("{SERVING}");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::internal(format!("no async runtime: {error}")))?;
    runtime.block_on(serve())
}

async fn serve() -> Result<(), CliError> {
    let running = DynoServer
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| CliError::internal(format!("the MCP session did not start: {error}")))?;
    running
        .waiting()
        .await
        .map_err(|error| CliError::internal(format!("the MCP server stopped: {error}")))?;
    Ok(())
}

#[tool_router]
impl DynoServer {
    #[tool(description = CHECK_DESCRIPTION)]
    async fn check(&self, Parameters(params): Parameters<CheckParams>) -> CallToolResult {
        tool_result(on_a_thread(move || run_check(params)).await)
    }

    #[tool(description = COMPARE_DESCRIPTION)]
    async fn compare(&self, Parameters(params): Parameters<CompareParams>) -> CallToolResult {
        tool_result(on_a_thread(move || run_compare(params)).await)
    }

    #[tool(description = READ_RESULT_DESCRIPTION)]
    async fn read_result(
        &self,
        Parameters(params): Parameters<ReadResultParams>,
    ) -> CallToolResult {
        tool_result(on_a_thread(move || run_read_result(params)).await)
    }
}

#[tool_handler]
impl ServerHandler for DynoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, TOOL_VERSION))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Runs `work` on its own thread and waits for its answer. The CDP driver starts its own async
/// runtime, which cannot start inside this one.
async fn on_a_thread(
    work: impl FnOnce() -> Result<ToolAnswer, CliError> + Send + 'static,
) -> Result<ToolAnswer, CliError> {
    let (sender, receiver) = oneshot::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver.await.unwrap_or_else(|_| {
        Err(CliError::internal(
            "the tool stopped before it gave an answer",
        ))
    })
}

/// A working tool's JSON and summary as two text blocks, or its error as a tool error.
fn tool_result(answer: Result<ToolAnswer, CliError>) -> CallToolResult {
    match answer {
        Ok(answer) => CallToolResult::success(vec![
            ContentBlock::text(answer.json),
            ContentBlock::text(answer.summary.join("\n")),
        ]),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }
}

fn run_check(params: CheckParams) -> Result<ToolAnswer, CliError> {
    let host_profile = named_host_profile(params.host_profile.as_deref())?;
    let options = CheckOptions::new(host_profile, params.budget.as_deref())?;
    let target = CheckTarget::new(params.build, params.launch_url.as_deref())?;
    let release = Release {
        game: params.game,
        version: params.release,
        game_id: None,
    };
    let result = check_target(&target, release, &options)?;
    let place = match &params.out {
        Some(out) => {
            output::write_json(&result, Some(out))?;
            ResultPlace::File(out)
        }
        None => ResultPlace::ToolResult,
    };
    Ok(ToolAnswer {
        json: canonical_json(&result)?,
        summary: output::check_summary(&result, place),
    })
}

fn run_compare(params: CompareParams) -> Result<ToolAnswer, CliError> {
    let compare = compare_files(&params.base, &params.head, params.budget.as_deref())?;
    Ok(ToolAnswer {
        json: canonical_json(&compare)?,
        summary: output::compare_summary(&compare),
    })
}

fn run_read_result(params: ReadResultParams) -> Result<ToolAnswer, CliError> {
    let loaded = read_result(&params.path)?;
    let result = &loaded.result;
    let status = ResultStatus {
        status: result.deterministic.status,
        result_hash: result.result_hash.clone(),
        hash_matches: loaded.hash_matches,
    };
    let mut summary = Vec::new();
    if !loaded.hash_matches {
        summary.push(EDITED.to_owned());
    }
    summary.extend(output::check_summary(
        result,
        ResultPlace::File(&params.path),
    ));
    Ok(ToolAnswer {
        json: canonical_json(&status)?,
        summary,
    })
}

/// The host profile with this name, or the default. An unknown name is a usage error.
fn named_host_profile(name: Option<&str>) -> Result<HostProfile, CliError> {
    let name = name.unwrap_or(DEFAULT_HOST_PROFILE);
    HostProfile::named(name).ok_or_else(|| {
        CliError::usage(format!(
            "unknown host profile {name:?}: use one of {}",
            HostProfile::NAMES.join(", ")
        ))
    })
}

fn canonical_json<T: Serialize>(value: &T) -> Result<String, CliError> {
    to_canonical_json(value).map_err(CliError::internal)
}
