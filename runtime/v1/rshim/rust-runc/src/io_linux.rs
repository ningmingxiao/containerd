use log::info;
use nix::unistd::pipe;
use nix::unistd::{fchown, Gid, Uid};
use std::option::Option;
use std::process::{Command, Stdio};
use std::{
    fs::File,
    fs::OpenOptions,
    os::unix::io::{FromRawFd, IntoRawFd, RawFd},
};

const DEVNULL: &str = "/dev/null";

pub trait Io {
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_mut_any(&mut self) -> &mut dyn std::any::Any;
    fn stdin(&self) -> Option<File>;
    fn stdout(&self) -> Option<File>;
    fn stderr(&self) -> Option<File>;
    fn set_process_io(&self, process: &mut Command);
}

#[derive(Debug, Clone, Copy)]
pub struct Pipe {
    pub r: RawFd,
    pub w: RawFd,
}

impl Pipe {
    pub fn new() -> Option<Self> {
        pipe().map(|(r, w)| Pipe { r, w }).ok()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PipeIO {
    pub stdin: Option<Pipe>,
    pub stdout: Option<Pipe>,
    pub stderr: Option<Pipe>,
}

#[derive(Debug, Clone, Copy)]
pub struct NullIO {
    pub dev_null: Option<RawFd>,
}

impl NullIO {}

pub fn new_pipe_io(
    s_in: String,
    s_out: String,
    s_err: String,
    uid: Uid,
    gid: Gid,
) -> Option<Box<dyn Io + Send + Sync>> {
    let mut stdin = None;
    let mut stdout = None;
    let mut stderr = None;

    if s_in != "".to_string() {
        stdin = Pipe::new().map(|sin| {
            match fchown(sin.r, Some(uid), Some(gid)) {
                Ok(_) => {}
                Err(e) => {
                    info!("fchown pipe stdin failed: {:?}", e);
                }
            };
            sin
        });
    }

    if s_out != "".to_string() {
        stdout = Pipe::new().map(|sout| {
            match fchown(sout.w, Some(uid), Some(gid)) {
                Ok(_) => {}
                Err(e) => {
                    info!("fchown pipe stdout failed: {:?}", e);
                }
            };
            sout
        });
    }

    if s_err != "".to_string() {
        stderr = Pipe::new().map(|serr| {
            match fchown(serr.w, Some(uid), Some(gid)) {
                Ok(_) => {}
                Err(e) => {
                    info!("fchown pipe stderr failed: {:?}", e);
                }
            };
            serr
        });
    }

    return Some(Box::new(PipeIO {
        stdin,
        stdout,
        stderr,
    }));
}

impl Io for PipeIO {
    fn as_any(&self) -> &dyn std::any::Any {
        return self;
    }

    fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
        return self;
    }

    fn stdin(&self) -> Option<File> {
        let mut console_in = None;
        if let Some(ref stdin) = self.stdin {
            console_in = Some(unsafe { File::from_raw_fd(stdin.w) });
        }
        return console_in;
    }

    fn stdout(&self) -> Option<File> {
        let mut console_out = None;
        if let Some(ref stdout) = self.stdout {
            console_out = Some(unsafe { File::from_raw_fd(stdout.r) });
        }
        return console_out;
    }

    fn stderr(&self) -> Option<File> {
        let mut console_err = None;
        if let Some(ref stderr) = self.stderr {
            console_err = Some(unsafe { File::from_raw_fd(stderr.r) });
        }
        return console_err;
    }

    fn set_process_io(&self, process: &mut Command) {
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(i) = &self.stdin {
            unsafe {
                let stdin_reader = Stdio::from_raw_fd(i.r);
                process.stdin(stdin_reader);
            }
        };

        if let Some(o) = &self.stdout {
            unsafe {
                let stdout_writer = Stdio::from_raw_fd(o.w);
                process.stdout(stdout_writer);
            }
        };

        if let Some(e) = &self.stderr {
            unsafe {
                let stderr_writer = Stdio::from_raw_fd(e.w);
                process.stderr(stderr_writer);
            }
        };
    }
}

pub fn new_null_io() -> Option<Box<dyn Io + Send + Sync>> {
    let mut file = None;
    if let Ok(f) = OpenOptions::new().read(true).write(true).open(DEVNULL) {
        file = Some(f.into_raw_fd());
    }

    return Some(Box::new(NullIO { dev_null: file }));
}

impl Io for NullIO {
    fn as_any(&self) -> &dyn std::any::Any {
        return self;
    }

    fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
        return self;
    }

    fn stdin(&self) -> Option<File> {
        return None;
    }

    fn stdout(&self) -> Option<File> {
        return None;
    }

    fn stderr(&self) -> Option<File> {
        return None;
    }

    fn set_process_io(&self, process: &mut Command) {
        if let Some(stdout) = &self.dev_null {
            process.stdout(unsafe { Stdio::from_raw_fd(*stdout) });
        }

        if let Some(stderr) = &self.dev_null {
            process.stderr(unsafe { Stdio::from_raw_fd(*stderr) });
        }
    }
}
