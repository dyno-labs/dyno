//! The dyno result shape.
//!
//! Every type that crosses a process or language boundary lives here: the check result, the
//! compare diff, the catalogue result, the doctor report, the budget and games files, the in-page
//! probe events, the runner protocol and the host's unclean exit record. The JSON Schema files in
//! `packages/schema/json/` and the TypeScript types in `packages/schema/ts/` are generated from
//! these types by [`generate`].
//!
//! After the first release, changes are additive. A breaking change bumps [`SCHEMA_VERSION`].

pub mod budget;
pub mod canonical;
pub mod catalogue;
pub mod compare;
pub mod config;
pub mod doctor;
pub mod generate;
pub mod host;
pub mod ids;
pub mod launch_url;
pub mod library;
pub mod manifest;
pub mod measure;
pub mod path;
pub mod probe;
pub mod profile;
pub mod result;
pub mod rule;
pub mod runner;
pub mod scrub;
pub mod status;

/// The version of every shape in this crate. It bumps only on a breaking change.
pub const SCHEMA_VERSION: u32 = 0;

/// The dyno version, such as "0.1.0". Every crate shares the workspace version.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
