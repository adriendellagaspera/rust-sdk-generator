use openapi_to_rust_bindings::read_bindings;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> &'static str {
    "usage: openapi-to-rust-bindings <generated-directory> <effective-openapi.json> | --extract <generated-directory> <effective-openapi.json>"
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

    let (directory, openapi) = match arguments.as_slice() {
        [directory, openapi] => (directory, openapi),
        [flag, directory, openapi] if flag == "--extract" => (directory, openapi),
        [directory] => {
            return Err(format!(
                "adapter.input.effective_openapi_required: supply the exact effective OpenAPI JSON as a second argument for {}",
                PathBuf::from(directory).display()
            ));
        }
        _ => return Err(usage().to_owned()),
    };

    let bindings = read_bindings(PathBuf::from(directory), PathBuf::from(openapi))
        .map_err(|error| format!("adapter.extract: {error}"))?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, &bindings).map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())
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
