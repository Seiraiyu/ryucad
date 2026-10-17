//! `ryucad-app`: the desktop app.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        let v = serde_json::json!(ryucad::proto::VersionInfo::current());
        println!("{v}");
        return ExitCode::SUCCESS;
    }
    eprintln!("usage: ryucad-app --version");
    ExitCode::from(2)
}
