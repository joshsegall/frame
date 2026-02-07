use clap::Parser;
use frame::cli::commands::Cli;
use frame::cli::handlers;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = handlers::dispatch(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
