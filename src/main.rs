fn main() {
    std::process::exit(rust_sdk_generator::cli::run_args(std::env::args().skip(1)));
}
