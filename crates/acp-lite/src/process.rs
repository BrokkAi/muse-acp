use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

/// How to launch an ACP agent over stdio.
#[derive(Debug, Clone)]
pub struct AgentProcess {
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
    pub(crate) envs: Vec<(OsString, OsString)>,
    pub(crate) cwd: Option<PathBuf>,
}

impl AgentProcess {
    #[must_use]
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
            cwd: None,
        }
    }

    pub fn arg(&mut self, arg: impl Into<OsString>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> &mut Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    pub fn current_dir(&mut self, directory: impl Into<PathBuf>) -> &mut Self {
        self.cwd = Some(directory.into());
        self
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .envs(self.envs.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = &self.cwd {
            command.current_dir(directory);
        }
        command
    }
}

#[derive(Debug, Default)]
pub(crate) struct StderrTail {
    lines: Mutex<VecDeque<String>>,
    capacity: usize,
}

impl StderrTail {
    pub(crate) fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        })
    }

    fn push(&self, line: String) {
        if self.capacity == 0 {
            return;
        }
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lines.len() == self.capacity {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub(crate) fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

pub(crate) fn drain_stderr<R: std::io::Read + Send + 'static>(
    stderr: R,
    tail: Arc<StderrTail>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(line.last(), Some(b'\n' | b'\r')) {
                        line.pop();
                    }
                    let end = line.len().min(8 * 1024);
                    let text = String::from_utf8_lossy(&line[..end]).into_owned();
                    tail.push(text);
                }
            }
        }
    })
}
