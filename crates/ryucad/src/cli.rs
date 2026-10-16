//! The agent CLI: arguments in, one JSON document out on stdout, and a
//! meaningful exit code.

use clap::{Parser, Subcommand};
use ryucad::proto::VersionInfo;
use serde_json::{Value, json};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "ryucad",
    about = "Agent-driven CAD",
    disable_version_flag = true
)]
struct Cli {
    /// Indent the JSON output.
    #[arg(long, global = true)]
    pretty: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the ryucad, ryukei and protocol versions.
    Version,
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Version => json!(VersionInfo::current()),
    };
    print(
        &json!({"ok": true, "result": result, "warnings": []}),
        cli.pretty,
    );
    ExitCode::SUCCESS
}

fn print(v: &Value, pretty: bool) {
    let s = if pretty {
        serde_json::to_string_pretty(v)
    } else {
        serde_json::to_string(v)
    };
    println!("{}", s.expect("JSON values always serialize"));
}
