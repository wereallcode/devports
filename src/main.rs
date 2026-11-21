use std::process::ExitCode;

fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && args[0] == "--version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if !args.is_empty() {
        eprintln!("devports takes no arguments except --version.");
        return ExitCode::from(2);
    }
    match devports::app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let message = devports::text::escape_controls(&error);
            eprintln!("devports: {message}");
            ExitCode::FAILURE
        }
    }
}
