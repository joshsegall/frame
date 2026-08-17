use clap::Parser;
use frame::cli::commands::{Cli, Commands};
use frame::cli::handlers;

fn main() {
    let cli = Cli::parse();
    let project_dir = cli.project_dir.clone();

    match cli.command {
        None => {
            // No subcommand → launch TUI
            if let Err(e) = frame::tui::run(project_dir.as_deref()) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Init(args)) => {
            // Init is handled before project discovery, so it arms the -C
            // override itself rather than inheriting it from `dispatch`.
            let json = cli.json;
            let result = handlers::set_project_dir_override(project_dir.as_deref())
                .and_then(|()| handlers::cmd_init(args, json));
            if let Err(e) = result {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
            // `dispatch` prints this for every other command; init does not go
            // through it.
            if !json && frame::io::dryrun::is_active() {
                handlers::print_dry_run_trailer();
            }
        }
        Some(Commands::Merge(args)) if args.resolve.is_empty() => {
            // A merge driver's exit status *is* its result — 0 merged, 1
            // conflicted, 2 declined — so it reports its own rather than being
            // flattened into the generic error path. Runs before project
            // discovery: it must neither lock the project nor register it.
            std::process::exit(handlers::cmd_merge(args));
        }
        Some(_) => {
            if let Err(e) = handlers::dispatch(cli) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
