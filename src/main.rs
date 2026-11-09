use anyhow::Result as AnyResult;
use clap::{Parser, Subcommand};

mod aur;
mod command;
mod review;
mod system;

#[derive(Parser, Debug)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Install {
        #[arg(short, long)]
        package: Option<String>,
    },
    Info {
        #[arg(short, long)]
        package: Option<String>,
    },
}

fn main() -> AnyResult<()> {
    let args = Args::parse();

    match args.command {
        Command::Info { package } => command::command_info(package.as_ref().map(|v| v.as_str()))?,
        Command::Install { package } => {
            command::command_install(package.as_ref().map(|v| v.as_str()))?
        }
    }

    Ok(())
}
