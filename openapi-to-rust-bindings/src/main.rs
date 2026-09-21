use openapi_to_rust_bindings::{
    extract_bindings, inspect_generated, inspect_semantics, read_bindings,
};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> &'static str {
    "usage: openapi-to-rust-bindings <generated-directory> | --inspect <generated-directory> | --inspect-semantics <generated-directory> <openapi.json> | --extract <generated-directory> <openapi.json>"
}

fn run() -> Result<(), String> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("openapi-to-rust-bindings {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h") {
        println!("{}", usage());
        return Ok(());
    }

    let stdout = io::stdout();
    let mut output = stdout.lock();
    match arguments.as_slice() {
        [directory] => {
            let bindings =
                read_bindings(PathBuf::from(directory)).map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(&mut output, bindings.as_value())
                .map_err(|error| error.to_string())?;
        }
        [flag, directory] if flag == "--inspect" => {
            let evidence =
                inspect_generated(PathBuf::from(directory)).map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(&mut output, &evidence)
                .map_err(|error| error.to_string())?;
        }
        [flag, directory, openapi] if flag == "--inspect-semantics" => {
            let evidence = inspect_semantics(PathBuf::from(directory), PathBuf::from(openapi))
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(&mut output, &evidence)
                .map_err(|error| error.to_string())?;
        }
        [flag, directory, openapi] if flag == "--extract" => {
            let bindings = extract_bindings(PathBuf::from(directory), PathBuf::from(openapi))
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(&mut output, bindings.as_value())
                .map_err(|error| error.to_string())?;
        }
        _ => return Err(usage().to_owned()),
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
