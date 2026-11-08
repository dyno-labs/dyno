//! `--help` for every command. Each command and flag names the plan phase that builds it.

mod common;

use common::dyno;

fn help(args: &[&str]) -> String {
    let outcome = dyno(args);
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    outcome.stdout
}

#[test]
fn dyno_help() {
    insta::assert_snapshot!("dyno", help(&["--help"]));
}

#[test]
fn check_help() {
    insta::assert_snapshot!("check", help(&["check", "--help"]));
}

#[test]
fn compare_help() {
    insta::assert_snapshot!("compare", help(&["compare", "--help"]));
}

#[test]
fn doctor_help() {
    insta::assert_snapshot!("doctor", help(&["doctor", "--help"]));
}

#[test]
fn mcp_help() {
    insta::assert_snapshot!("mcp", help(&["mcp", "--help"]));
}

#[test]
fn login_help() {
    insta::assert_snapshot!("login", help(&["login", "--help"]));
}

#[test]
fn logout_help() {
    insta::assert_snapshot!("logout", help(&["logout", "--help"]));
}

#[test]
fn upload_help() {
    insta::assert_snapshot!("upload", help(&["upload", "--help"]));
}
