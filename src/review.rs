use std::io::Write;
use std::{env, io, thread};

use anyhow::{Result as AnyResult, anyhow};

use crate::system;

pub fn run_pager<'a>(
    mut data: impl io::Read + Send,
    args: impl IntoIterator<Item = &'a str>,
) -> AnyResult<()> {
    let pager = env::var("PAGER").unwrap_or("less".to_string());

    let args: Vec<String> = [pager]
        .into_iter()
        .chain(args.into_iter().map(|v| v.to_string()))
        .collect();

    system::run(
        None,
        args.iter().map(|v| v.as_str()),
        system::StdioData::Reader(&mut data),
        system::StdioData::Inherit,
        system::StdioData::Inherit,
    )?;
    Ok(())
}

pub fn run_pager_git<'a>(dir: &str, mut data: impl io::Read + Send) -> AnyResult<()> {
    thread::scope(|s| {
        if system::is_executable("delta")? {
            let (delta_reader, mut delta_writer) = io::pipe()?;
            let delta_worker = s.spawn(move || {
                system::run(
                    Some(dir),
                    ["delta"],
                    system::StdioData::Reader(&mut data),
                    system::StdioData::Writer(&mut delta_writer),
                    system::StdioData::Null,
                )
            });

            let pager_worker = s.spawn(|| run_pager(delta_reader, ["-R"]));

            pager_worker.join().expect("no panic")?;
            delta_worker.join().expect("no panic")?;
        } else {
            let pager_worker = s.spawn(|| run_pager(data, []));
            pager_worker.join().expect("no panic")?;
        }

        anyhow::Ok(())
    })?;
    Ok(())
}

pub fn run_git_repo_diff_all(repo_directory: &str, writer: impl io::Write + Send) -> AnyResult<()> {
    system::run_writer(
        Some(repo_directory),
        [
            "git",
            "diff",
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904..HEAD",
        ],
        writer,
    )?;

    Ok(())
}

pub fn ask_for_permission(action: &str) -> AnyResult<()> {
    print!("Proceed with {} [Y/n]? ", action);
    io::stdout().flush()?;

    let choice = system::with_exit(|| {
        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;
        anyhow::Ok(choice)
    })??;
    let choice = choice.trim().to_lowercase();

    if choice.is_empty() || choice == "y" {
        return Ok(());
    }

    Err(anyhow!("user declined action: {}", action))
}
