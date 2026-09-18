use openapi_to_rust_bindings::read_bindings;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> &'static str {
    "usage: openapi-to-rust-bindings <generated-directory>"
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let Some(argument) = arguments.next() else {
        return Err(usage().to_owned());
    };
    if argument == "--version" {
        if arguments.next().is_some() {
            return Err(usage().to_owned());
        }
        println!("openapi-to-rust-bindings {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if argument == "--help" || argument == "-h" {
        println!("{}", usage());
        return Ok(());
    }
    if arguments.next().is_some() {
        return Err(usage().to_owned());
    }

    let bindings = read_bindings(PathBuf::from(argument)).map_err(|error| error.to_string())?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, bindings.as_value())
        .map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
