use openapi_to_rust_bindings::{inspect_generated, read_bindings};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> &'static str {
    "usage: openapi-to-rust-bindings [--inspect] <generated-directory>"
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let first = arguments.next();
    let inspect = first
        .as_ref()
        .is_some_and(|argument| argument == "--inspect");
    let Some(argument) = (if inspect { arguments.next() } else { first }) else {
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

    let stdout = io::stdout();
    let mut output = stdout.lock();
    if inspect {
        let evidence =
            inspect_generated(PathBuf::from(argument)).map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut output, &evidence).map_err(|error| error.to_string())?;
    } else {
        let bindings = read_bindings(PathBuf::from(argument)).map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut output, bindings.as_value())
            .map_err(|error| error.to_string())?;
    }
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
