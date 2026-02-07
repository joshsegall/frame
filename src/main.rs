use clap::Parser;
use frame::cli::commands::{Cli, Commands};
use frame::cli::handlers;

fn main() {
    let cli = Cli::parse();
    match cli.command {
        None => {
            // No subcommand → launch TUI
            if let Err(e) = frame::tui::run() {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Init(args)) => {
            // Init is handled before project discovery
            if let Err(e) = handlers::cmd_init(args) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some(_) => {
            if let Err(e) = handlers::dispatch(cli) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
