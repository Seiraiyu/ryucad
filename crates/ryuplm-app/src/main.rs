//! `ryuplm-app [DIR]`: ryuplm's window on the catalogue in or above DIR.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        let v = serde_json::json!({"ok": true, "result": ryuplm::proto::VersionInfo::current(), "warnings": []});
        println!("{v}");
        return ExitCode::SUCCESS;
    }
    eprintln!("ryuplm-app: not yet");
    ExitCode::from(2)
}
