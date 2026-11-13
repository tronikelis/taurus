use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::Duration;
use std::{env, error, fmt, io, thread};

use anyhow::{Result as AnyResult, anyhow};
use nix::sys::signal;
use nix::unistd;

use crate::EXIT;

pub static PENDING_CHILD_IDS: LazyLock<Mutex<HashSet<u32>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn kill_pending_child_ids() {
    for id in PENDING_CHILD_IDS.lock().expect("lock not poisoned").iter() {
        let _ = signal::kill(unistd::Pid::from_raw(*id as _), signal::SIGTERM);
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct ChildKillGuard {
    id: u32,
}

impl ChildKillGuard {
    fn new(child: &Child) -> Self {
        let id = child.id();
        PENDING_CHILD_IDS
            .lock()
            .expect("lock not poisoned")
            .insert(id);
        Self { id }
    }
}

impl Drop for ChildKillGuard {
    fn drop(&mut self) {
        PENDING_CHILD_IDS
            .lock()
            .expect("lock not poisoned")
            .remove(&self.id);
    }
}

#[derive(Debug)]
pub struct ExitCodeError {
    pub code: i32,
}

impl fmt::Display for ExitCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "exit code {} is not 0", &self.code)
    }
}

impl error::Error for ExitCodeError {}

pub enum StdioData<'a> {
    Writer(&'a mut (dyn io::Write + Send)),
    Reader(&'a mut (dyn io::Read + Send)),
    Null,
    Inherit,
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
    let _guard = ChildKillGuard::new(&child);

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
        return Err(ExitCodeError {
            code: status.code().unwrap_or(-1),
        }
        .into());
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

/// Closure that aborts when the global crate::EXIT signal is true
pub fn with_exit<T: Send + 'static, F: FnOnce() -> T + Send + 'static>(func: F) -> AnyResult<T> {
    let finish = Arc::new(AtomicBool::new(false));

    let (tx, rx) = mpsc::sync_channel(1);

    thread::spawn({
        let tx = tx.clone();
        let finish = finish.clone();
        move || {
            let _ = tx.send(Some(func()));
            finish.store(true, atomic::Ordering::Relaxed);
        }
    });
    thread::spawn(move || {
        while !finish.load(atomic::Ordering::Relaxed) && !EXIT.load(atomic::Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(1));
        }
        let _ = tx.send(None);
    });

    Ok(rx.recv()?.ok_or(anyhow!("closure was aborted"))?)
}

#[derive(Debug)]
pub struct FileGuard {
    path: PathBuf,
    dismissed: bool,
}

impl FileGuard {
    pub fn new(path: impl AsRef<Path>) -> AnyResult<Self> {
        Ok(Self {
            dismissed: false,
            path: path.as_ref().to_path_buf(),
        })
    }

    pub fn get(&self) -> AnyResult<Option<String>> {
        if !self.path.exists() {
            return Ok(None);
        }

        let mut string = String::new();
        File::open(&self.path)?.read_to_string(&mut string)?;
        Ok(Some(string))
    }

    pub fn set(&self, contents: &str) -> AnyResult<()> {
        File::create(&self.path)?.write_all(contents.as_bytes())?;
        Ok(())
    }

    pub fn dismiss(mut self) {
        self.dismissed = true;
    }
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        if self.dismissed {
            let _ = fs::remove_file(&self.path);
        }
    }
}
