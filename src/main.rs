use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

use anyhow::Result as AnyResult;
use clap::{Parser, Subcommand};
use nix::sys::signal;
use signal_hook::iterator::Signals;

mod aur;
mod command;
mod review;
mod system;

static EXIT: AtomicBool = AtomicBool::new(false);

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

#[derive(Debug)]
enum AppEvent {
    Finish(AnyResult<()>),
    Terminate,
}

fn start_app(tx: SyncSender<AppEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        fn start() -> AnyResult<()> {
            let args = Args::parse();

            match args.command {
                Command::Info { package } => {
                    command::command_info(package.as_ref().map(|v| v.as_str()))?
                }
                Command::Install { package } => {
                    command::command_install(package.as_ref().map(|v| v.as_str()))?
                }
            }
            Ok(())
        }

        let _ = tx.send(AppEvent::Finish(start()));
    })
}

fn start_signals(tx: SyncSender<AppEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        for _ in Signals::new([signal::SIGTERM as i32, signal::SIGINT as i32])
            .expect("Signals::new")
            .forever()
        {
            system::kill_pending_child_ids();
            let _ = tx.send(AppEvent::Terminate);
            break;
        }
    })
}

fn main() -> AnyResult<()> {
    let (tx, rx) = mpsc::sync_channel::<AppEvent>(1);
    let app = start_app(tx.clone());
    start_signals(tx);

    loop {
        match rx.recv()? {
            AppEvent::Terminate => {
                println!();
                EXIT.store(true, atomic::Ordering::Relaxed);
            }
            AppEvent::Finish(v) => {
                app.join().expect("no panic");
                return v;
            }
        };
    }
}
