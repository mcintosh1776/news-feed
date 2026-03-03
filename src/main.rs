mod cli;
mod config;
mod discovery;
mod feed_parser;
#[cfg(feature = "gui")]
mod gui;
mod storage;
mod syncer;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let args = cli::CliArgs::parse();
    let run_gui = args.command.is_none() || matches!(args.command.as_ref(), Some(cli::Commands::Gui));

    if run_gui {
        #[cfg(feature = "gui")]
        {
            let db_path = config::resolve_db_path(if args.db.is_empty() {
                None
            } else {
                Some(std::path::Path::new(&args.db))
            })?;
            gui::run_gui(&db_path, args.interval_minutes, args.start_minimized)?;
            Ok(())
        }
        #[cfg(not(feature = "gui"))]
        Err(anyhow::anyhow!(
            "GUI mode requires building with --features gui (not supported in this binary)."
        ))
    } else {
        let cli_args = args;
        cli::run(cli_args)
    }
}
