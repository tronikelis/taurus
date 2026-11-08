use anyhow::Result as AnyResult;
use clap::{Parser, Subcommand};

mod aur;
mod command;
mod review;
mod system;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    package: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Install,
    Info,
}

fn main() -> AnyResult<()> {
    let args = Args::parse();

    match args.command {
        Command::Info => command::command_info(&args)?,
        Command::Install => command::command_install(&args)?,
    }

    Ok(())
}
