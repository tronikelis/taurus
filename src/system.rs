use std::env;
use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use anyhow::{Result as AnyResult, anyhow};

pub enum StdioData<'a> {
    Writer(&'a mut (dyn io::Write + Send)),
    Reader(&'a mut (dyn io::Read + Send)),
    Null,
    Inherit,
}

pub fn run_reader<'a>(
    wd: Option<&str>,
    args: impl IntoIterator<Item = &'a str>,
    mut stdin: impl io::Read + Send,
) -> AnyResult<()> {
    run(
        wd,
        args,
        StdioData::Reader(&mut stdin),
        StdioData::Null,
        StdioData::Null,
    )
}

pub fn run_writer<'a>(
    wd: Option<&str>,
    args: impl IntoIterator<Item = &'a str>,
    mut stdout: impl io::Write + Send,
) -> AnyResult<()> {
    run(
        wd,
        args,
        StdioData::Null,
        StdioData::Writer(&mut stdout),
        StdioData::Null,
    )
}

pub fn run_null<'a>(wd: Option<&str>, args: impl IntoIterator<Item = &'a str>) -> AnyResult<()> {
    run(wd, args, StdioData::Null, StdioData::Null, StdioData::Null)
}

pub fn run_inherit<'a>(wd: Option<&str>, args: impl IntoIterator<Item = &'a str>) -> AnyResult<()> {
    run(
        wd,
        args,
        StdioData::Inherit,
        StdioData::Inherit,
        StdioData::Inherit,
    )
}

pub fn run<'a>(
    wd: Option<&str>,
    args: impl IntoIterator<Item = &'a str>,
    mut stdin: StdioData<'_>,
    mut stdout: StdioData<'_>,
    mut stderr: StdioData<'_>,
) -> AnyResult<()> {
    let mut args_iter = args.into_iter();

    let command = args_iter
        .next()
        .ok_or_else(|| anyhow!("expected command to exist"))?;

    let mut child = Command::new(command);
    child
        .current_dir(wd.unwrap_or(&env::var("PWD")?))
        .args(args_iter);

    match &stdin {
        StdioData::Null => child.stdin(Stdio::null()),
        StdioData::Inherit => child.stdin(Stdio::inherit()),
        StdioData::Reader(_) => child.stdin(Stdio::piped()),
        StdioData::Writer(_) => return Err(anyhow!("writer not supported for stdin")),
    };
    match &stdout {
        StdioData::Null => child.stdout(Stdio::null()),
        StdioData::Inherit => child.stdout(Stdio::inherit()),
        StdioData::Writer(_) => child.stdout(Stdio::piped()),
        StdioData::Reader(_) => return Err(anyhow!("reader not supported for stdout")),
    };
    match &stderr {
        StdioData::Null => child.stderr(Stdio::null()),
        StdioData::Inherit => child.stderr(Stdio::inherit()),
        StdioData::Writer(_) => child.stderr(Stdio::piped()),
        StdioData::Reader(_) => return Err(anyhow!("reader not supported for stderr")),
    };

    let mut child = child.spawn()?;

    let child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();
    let status: ExitStatus = thread::scope(|scope| {
        let stdin_worker = scope.spawn(|| match (child_stdin, &mut stdin) {
            (Some(mut child_stdin), StdioData::Reader(stdin)) => {
                io::copy(stdin, &mut child_stdin)?;
                anyhow::Ok(())
            }
            _ => anyhow::Ok(()),
        });
        let stdout_worker = scope.spawn(|| match (child_stdout, &mut stdout) {
            (Some(mut child_stdout), StdioData::Writer(stdout)) => {
                io::copy(&mut child_stdout, stdout)?;
                anyhow::Ok(())
            }
            _ => anyhow::Ok(()),
        });
        let stderr_worker = scope.spawn(|| match (child_stderr, &mut stderr) {
            (Some(mut child_stderr), StdioData::Writer(stderr)) => {
                io::copy(&mut child_stderr, stderr)?;
                anyhow::Ok(())
            }
            _ => anyhow::Ok(()),
        });

        let wait_worker = scope.spawn(|| child.wait());

        let status = wait_worker.join().expect("no panic")?;
        stdin_worker.join().expect("no panic")?;
        stdout_worker.join().expect("no panic")?;
        stderr_worker.join().expect("no panic")?;
        anyhow::Ok(status)
    })?;

    if !status.success() {
        return Err(anyhow!(
            "exit code {} was not 0",
            status.code().unwrap_or(-1)
        ));
    }

    Ok(())
}

pub fn is_executable(name: &str) -> AnyResult<bool> {
    for dir in env::var("PATH")?.split(":") {
        if Path::new(&format!("{}/{}", dir, name)).exists() {
            return Ok(true);
        }
    }

    Ok(false)
}
