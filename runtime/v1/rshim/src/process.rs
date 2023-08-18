use crate::console::{copy_console, copy_pipes};
use crate::errors::{ttrpc_error, Result, ShimError};
use crate::mount_linux;
use crate::state::{self, ExecState, InitState};
use crate::types;
use anyhow::anyhow;
use anyhow::Context;
use chrono::{DateTime, Local};
use log::{error, info};
use nix::errno::Errno;
use nix::sys::signal::{kill, Signal};
use nix::unistd::{Gid, Pid, Uid};
use protocols::{runc, shim};
use rust_runc::{
    console::ReceivePtyMaster, io_linux::new_null_io, io_linux::new_pipe_io, io_linux::Io,
    io_linux::NullIO, io_linux::PipeIO, specs::LinuxResources, specs::Process, CreateOpts,
    DeleteOpts, ExecOpts, ExitInfo, KillOpts, Runc, RuncConfiguration, RuncLogFormat,
};
use std::convert::TryFrom;
use std::default::Default;
use std::os::unix::{
    fs::OpenOptionsExt,
    io::{AsRawFd, RawFd},
};
use std::sync::{Arc, Condvar, Mutex};
use std::{
    env,
    fs::{self, File, OpenOptions},
    path::PathBuf,
    thread::JoinHandle,
};
use ttrpc::Code;
nix::ioctl_write_ptr_bad!(set_term_size_unsafe, libc::TIOCSWINSZ, nix::pty::Winsize);

#[derive(Debug, Clone)]
pub struct Stdio {
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
    pub terminal: bool,
}

#[derive(Debug, Default)]
pub struct WinSize {
    pub height: u16,
    pub width: u16,
    pub x: u16,
    pub y: u16,
}

pub trait RuncProcess {
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_mut_any(&mut self) -> &mut dyn std::any::Any;
    fn id(&self) -> &str;
    fn pid(&self) -> u32;
    fn container_id(&self) -> &str;
    fn start(&mut self) -> Result<()>;
    fn state(&self) -> Result<String>;
    fn stdio(&self) -> Stdio;
    fn close_stdin(&mut self) -> Result<()>;
    fn delete(&mut self) -> Result<()>;
    fn kill(&self, sig: u32, all: bool) -> Result<()>;
    fn resize(&self, ws: &WinSize);
    fn set_exited(&mut self, status: i32);
    fn get_exit_status(&self) -> i32;
    fn get_exited_at(&self) -> Option<DateTime<Local>>;
    fn wait(&self);
    fn get_wait_pair(&self) -> Arc<(Mutex<bool>, Condvar)>;
}

pub struct InitProcess {
    pub id: String,
    container_id: String,
    state: InitState,
    // TODO: work_dir is used in checkpoint.
    #[allow(dead_code)]
    work_dir: String,
    pub bundle: String,
    console: Option<RawFd>,
    // Platform     proc.Platform
    io: Option<Box<dyn Io + Send + Sync>>,
    runtime: Arc<Runc>,
    status: i32,
    exited: Option<DateTime<Local>>,
    pub pid: i32,
    // closers      []io.Closer
    stdin: Option<File>,
    stdio: Stdio,
    // TODO: rootfs is currently not support, I don't know how to verify it.
    #[allow(dead_code)]
    rootfs: String,
    io_uid: u32,
    io_gid: u32,
    no_pivot_root: bool,
    no_new_keyring: bool,
    wait_pair: Arc<(Mutex<bool>, Condvar)>,
}

pub struct ExecProcess {
    // wg sync.WaitGroup
    state: ExecState,

    // mu      sync.Mutex
    // id      string
    // console console.Console
    // io      *processIO
    // status  int
    // exited  time.Time
    // pid     safePid
    // closers []io.Closer
    // stdin   io.Closer
    // stdio   stdio.Stdio
    // path    string
    // spec    specs.Process

    // parent    *Init
    // waitBlock chan struct{}
    pub id: String,
    pub pid: Mutex<i32>,
    container_id: String,
    console: Option<RawFd>,
    io: Option<Box<dyn Io + Send + Sync>>,
    io_uid: u32,
    io_gid: u32,
    status: i32,
    exited: Option<DateTime<Local>>,
    stdio: Stdio,
    stdin: Option<File>,
    runtime: Arc<Runc>,
    path: PathBuf,
    spec: Process,
    wait_pair: Arc<(Mutex<bool>, Condvar)>,
}

fn new_runc(
    root: &str,
    path: &str,
    namespace: &str,
    runtime: &str,
    _criu: &str,
    systemd: bool,
    exits: &Arc<ExitInfo>,
) -> Result<Runc> {
    let mut config = RuncConfiguration::default();
    if runtime != "runc" {
        config.command = Some(PathBuf::from(runtime));
    }
    config.root = Some(PathBuf::from(root).join(namespace));
    config.log = Some(PathBuf::from(path).join("log.json"));
    config.log_format = Some(RuncLogFormat::Json);
    // TODO: add criu
    config.systemd_cgroup = systemd;

    Ok(Runc::new(config, exits).context("unable to create runc instance")?)
}

impl ExecProcess {}

fn has_no_io(stdio: &Stdio) -> bool {
    stdio.stdin == "" && stdio.stdout == "" && stdio.stderr == ""
}

impl InitProcess {
    pub fn new(
        path: &str,
        work_dir: &str,
        runtime_root: &str,
        namespace: &str,
        criu: &str,
        systemd_cgroup: bool,
        req: &shim::CreateTaskRequest,
        rootfs: &str,
        exits: &Arc<ExitInfo>,
    ) -> Result<Self> {
        let mut op = req.get_options().clone();
        op.set_type_url(format!("{}/{}", "type.googleapis.com", op.type_url));

        let options = match op.unpack::<runc::CreateOptions>() {
            Ok(o) => match o {
                Some(o) => o,
                None => runc::CreateOptions::default(),
            },
            Err(error) => Err(error).context("failed to unpack Any message")?,
        };

        let stdio = Stdio {
            stdin: req.stdin.clone(),
            stdout: req.stdout.clone(),
            stderr: req.stderr.clone(),
            terminal: req.terminal,
        };

        let runtime = Arc::new(new_runc(
            runtime_root,
            path,
            namespace,
            &req.runtime,
            criu,
            systemd_cgroup,
            exits,
        )?);

        Ok(InitProcess {
            id: req.id.clone(),
            container_id: req.id.clone(),
            work_dir: String::from(work_dir),
            state: InitState::created(),
            bundle: req.bundle.clone(),
            io: None,
            console: None,
            runtime,
            status: 0,
            exited: None,
            pid: 0,
            stdio,
            stdin: None,
            rootfs: String::from(rootfs),
            io_uid: options.io_uid,
            io_gid: options.io_gid,
            no_pivot_root: options.no_pivot_root,
            no_new_keyring: options.no_new_keyring,
            wait_pair: Arc::new((Mutex::new(false), Condvar::new())),
        })
    }

    pub fn ps(&self) -> Result<Vec<usize>> {
        let r = self.runtime.ps(&self.id).context("OCI runtime ps failed")?;
        Ok(r)
    }

    pub fn create(&mut self) -> Result<()> {
        info!("create init process");
        // TODO: use mktemp to generate a temp dir?
        let mut console_socket = None;
        let mut receive_pty_master = None;
        let stdio = self.stdio.clone();
        let has_no_io = has_no_io(&stdio);
        let mut pipe_io = None;
        let mut null_io = None;

        if self.stdio.terminal {
            info!("self.stdio.terminal");
            let console_socket_path = env::temp_dir().join(&self.id).with_extension("console");
            if console_socket_path.exists() {
                info!("remove residual console socket {:?}", console_socket_path);
                fs::remove_file(&console_socket_path).with_context(|| {
                    format!("failed to remove console socket {:?}", console_socket_path)
                })?;
            }
            console_socket = Some(console_socket_path);
            info!("console_socket: {:?}", console_socket);
            receive_pty_master = Some(
                ReceivePtyMaster::new(
                    &console_socket
                        .clone()
                        .context("failed to clone console socket")?,
                )
                .context("unix socket should be created ok")?,
            );
            info!("new console pty, start to spawn");
        } else if has_no_io {
            // null IO
            self.io = new_null_io();
            if let Some(io) = &self.io {
                null_io = io.as_any().downcast_ref::<NullIO>().cloned();
            }
        } else {
            info!("else to do");
        }

        // pipe IO
        if !stdio.terminal && !has_no_io {
            info!("!stdio.terminal && !has_no_io");
            let stdio_clone = stdio.clone();
            self.io = new_pipe_io(
                stdio_clone.stdin,
                stdio_clone.stdout,
                stdio_clone.stderr,
                Uid::from_raw(self.io_uid),
                Gid::from_raw(self.io_gid),
            );
            if let Some(io) = &self.io {
                pipe_io = io.as_any().downcast_ref::<PipeIO>().cloned();
            }
        }

        let receive_pty_task = std::thread::spawn(move || {
            if stdio.terminal {
                info!("std::thread::spawn(.stdio.terminal");
                if let Some(server) = receive_pty_master {
                    match server.receive() {
                        Ok(mut pty_master) => {
                            info!("std::thread::spawn pty_master");
                            info!("{:?}", pty_master);
                            let pty_fd = pty_master.as_raw_fd();
                            let _: JoinHandle<std::result::Result<(), ShimError>> =
                                std::thread::spawn(move || {
                                    copy_console(&mut pty_master, &stdio.stdin, &stdio.stdout)
                                        .context("failed to copy console")?;
                                    Ok(())
                                });
                            info!("copy_console ok");
                            Some(pty_fd)
                        }
                        Err(err) => {
                            error!("Receive PTY master error: {}", err);
                            None
                        }
                    }
                } else {
                    info!("std::thread::spawn None");
                    None
                }
            } else {
                if !has_no_io {
                    info!("std::thread::spawn if !has_no_io");
                    let _: JoinHandle<std::result::Result<(), ShimError>> =
                        std::thread::spawn(move || {
                            info!("");
                            if let Some(io) = pipe_io {
                                info!("std::thread::spawn if copy_pipes");
                                copy_pipes(&io, &stdio.stdin, &stdio.stdout, &stdio.stderr)
                                    .context("failed to copy pipe")?;
                                info!("copy pipe finished");
                            }
                            Ok(())
                        });
                }
                info!("std::thread::spawn if None");
                None
            }
        });

        let pid_file = PathBuf::from(&self.bundle).join("init.pid");
        // fs::File::create(&pid_file).await.unwrap();
        info!("pid_fild: {:?}", &pid_file);
        // TODO: set myself as subscreaper
        if has_no_io {
            self.runtime
                .create(
                    &self.id,
                    &PathBuf::from(&self.bundle),
                    Some(&CreateOpts {
                        pid_file: Some(pid_file.clone()),
                        console_socket,
                        no_pivot: self.no_pivot_root,
                        no_new_keyring: self.no_new_keyring,
                        detach: false,
                    }),
                    null_io,
                )
                .context("OCI runtime create failed")?;
        } else {
            self.runtime
                .create(
                    &self.id,
                    &PathBuf::from(&self.bundle),
                    Some(&CreateOpts {
                        pid_file: Some(pid_file.clone()),
                        console_socket,
                        no_pivot: self.no_pivot_root,
                        no_new_keyring: self.no_new_keyring,
                        detach: false,
                    }),
                    pipe_io,
                )
                .context("OCI runtime create failed")?;
        }

        info!("after pid_file!");

        if self.stdio.stdin != "" {
            self.stdin = Some(
                OpenOptions::new()
                    .write(true)
                    .read(false)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&self.stdio.stdin)
                    .context("failed to open stdin")?,
            );
        }

        info!("create ok!");
        self.pid = std::fs::read_to_string(pid_file)
            .context("failed to read pid file")?
            .parse()
            .context("failed to parse pid")?;
        info!("read pid: {}", &self.pid);

        if self.stdio.terminal {
            self.console = receive_pty_task.join().unwrap();
        }

        info!("run done!");
        Ok(())
    }

    pub fn exec(&self, path: &str, r: types::ExecConfig) -> Result<ExecProcess> {
        if !(self.state == InitState::running() || self.state == InitState::created()) {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot start a {:?} process", self.state),
            )
            .into());
        }

        let mut spec: Process =
            serde_json::from_slice(r.spec.get_value()).context("failed to get spec")?;
        info!("spec: {:?}", spec);

        spec.terminal = Some(r.terminal);

        let stdio = Stdio {
            stdin: r.stdin,
            stdout: r.stdout,
            stderr: r.stderr,
            terminal: r.terminal,
        };

        Ok(ExecProcess {
            id: r.id,
            container_id: self.container_id.clone(),
            state: ExecState::exec_created(),
            console: None,
            io: None,
            io_uid: self.io_uid,
            io_gid: self.io_gid,
            status: 0,
            exited: None,
            pid: Mutex::new(0),
            stdio,
            stdin: None,
            runtime: Arc::clone(&self.runtime),
            path: PathBuf::from(path),
            spec,
            wait_pair: Arc::new((Mutex::new(false), Condvar::new())),
        })
    }

    pub fn kill_all(&self) -> anyhow::Result<()> {
        self.runtime
            .kill(
                self.id(),
                Signal::SIGKILL as i32,
                Some(&KillOpts { all: true }),
            )
            .context("OCI runtime killall failed")?;

        Ok(())
    }

    pub fn pause(&mut self) -> Result<()> {
        if self.state == InitState::running() {
            self.runtime
                .pause(self.id())
                .context("OCI runtime pause failed")?;
            self.state = self.state.on_pause(state::Pause);
            Ok(())
        } else {
            Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot pause a {:?} process", self.state),
            )
            .into())
        }
    }

    pub fn resume(&mut self) -> Result<()> {
        if self.state == InitState::paused() {
            self.runtime
                .resume(self.id())
                .context("OCI runtime resume failed")?;
            self.state = self.state.on_start(state::Start);
            Ok(())
        } else {
            Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot resume a {:?} process", self.state),
            )
            .into())
        }
    }

    pub fn update(&self, resources: &LinuxResources) -> Result<()> {
        if self.state == InitState::created()
            || self.state == InitState::created_checkpoint()
            || self.state == InitState::running()
            || self.state == InitState::paused()
        {
            self.runtime
                .update(&self.id, resources)
                .context("OCI runtime update failed")?;
            Ok(())
        } else {
            Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot update a {:?} process", self.state),
            )
            .into())
        }
    }

    #[allow(dead_code)]
    pub fn checkpoint(&self) -> Result<()> {
        if self.state == InitState::running() || self.state == InitState::paused() {
            // TODO: checkpoint
            // self.runtime.checkpoint()
            Ok(())
        } else {
            Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot checkpoint a {:?} process", self.state),
            )
            .into())
        }
    }
}

impl RuncProcess for ExecProcess {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn pid(&self) -> u32 {
        let pid = self.pid.lock().unwrap();
        *pid as u32
    }

    fn container_id(&self) -> &str {
        &self.container_id
    }

    fn resize(&self, ws: &WinSize) {
        let ws = nix::pty::Winsize {
            ws_row: ws.height,
            ws_col: ws.width,
            ws_xpixel: ws.x,
            ws_ypixel: ws.y,
        };

        if let Some(console) = self.console {
            unsafe { set_term_size_unsafe(console, &ws as *const nix::pty::Winsize) }
                .unwrap_or_else(|e| {
                    info!("set term size failed: {:?}", e);
                    0
                });
        }
    }

    fn start(&mut self) -> Result<()> {
        if self.state != ExecState::exec_created() {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot start a {:?} process", self.state),
            )
            .into());
        }

        info!("start exec tty!");
        let stdio = self.stdio.clone();
        let has_no_io = has_no_io(&self.stdio);
        let mut pid = self.pid.lock().unwrap();
        let pid_file = self.path.join(format!("{}.pid", self.id));

        let mut console_socket = None;
        let mut receive_pty_master = None;

        let mut pipe_io = None;
        let mut null_io = None;

        if self.stdio.terminal {
            let console_socket_path = env::temp_dir().join(&self.id).with_extension("console");
            if console_socket_path.exists() {
                info!("remove residual console socket {:?}", console_socket_path);
                fs::remove_file(&console_socket_path).with_context(|| {
                    format!("failed to remove console socket {:?}", console_socket_path)
                })?;
            }
            console_socket = Some(console_socket_path);
            info!("console_socket: {:?}", console_socket);
            receive_pty_master = Some(
                ReceivePtyMaster::new(
                    &console_socket
                        .clone()
                        .context("failed to clone console socket")?,
                )
                .context("unix socket should be created ok")?,
            );
            info!("new console pty, start to spawn");
        } else if has_no_io {
            // null_io
            self.io = new_null_io();
            if let Some(io) = &self.io {
                null_io = io.as_any().downcast_ref::<NullIO>().cloned();
            }
        } else {
        }

        // pipe IO
        if !stdio.terminal && !has_no_io {
            info!("!stdio.terminal && !has_no_io");
            let stdio_clone = stdio.clone();
            self.io = new_pipe_io(
                stdio_clone.stdin,
                stdio_clone.stdout,
                stdio_clone.stderr,
                Uid::from_raw(self.io_uid),
                Gid::from_raw(self.io_gid),
            );
            if let Some(io) = &self.io {
                pipe_io = io.as_any().downcast_ref::<PipeIO>().cloned();
            }
        }

        let receive_pty_task = std::thread::spawn(move || {
            if stdio.terminal {
                if let Some(server) = receive_pty_master {
                    match server.receive() {
                        Ok(mut pty_master) => {
                            info!("{:?}", pty_master);
                            let pty_fd = pty_master.as_raw_fd();
                            let _: JoinHandle<std::result::Result<(), ShimError>> =
                                std::thread::spawn(move || {
                                    copy_console(&mut pty_master, &stdio.stdin, &stdio.stdout)
                                        .context("failed to copy console")?;
                                    Ok(())
                                });
                            info!("copy_console ok");
                            Some(pty_fd)
                        }
                        Err(err) => {
                            error!("Receive PTY master error: {}", err);
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                if !has_no_io {
                    let _: JoinHandle<std::result::Result<(), ShimError>> =
                        std::thread::spawn(move || {
                            info!("");
                            if let Some(io) = pipe_io {
                                copy_pipes(&io, &stdio.stdin, &stdio.stdout, &stdio.stderr)
                                    .context("failed to copy pipe")?;
                                info!("copy pipe finished");
                            }
                            Ok(())
                        });
                }

                None
            }
        });
        info!("receive_pty_task create ok");

        info!("pid_file: {:?}", pid_file);
        let opts = ExecOpts {
            pid_file: Some(pid_file),
            console_socket,
            detach: true,
        };

        info!("opts ok");

        if has_no_io {
            self.runtime
                .exec(&self.container_id, &self.spec, Some(&opts), null_io)
                .context("OCI runtime exec failed")?;
        } else {
            self.runtime
                .exec(&self.container_id, &self.spec, Some(&opts), pipe_io)
                .context("OCI runtime exec failed")?;
        }

        if self.stdio.stdin != "" {
            info!("open begin: {}", self.stdio.stdin);
            self.stdin = OpenOptions::new()
                .write(true)
                .read(false)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.stdio.stdin)
                .ok();
        }

        info!("exec ok!");
        *pid = std::fs::read_to_string(opts.pid_file.context("failed to get pid file")?)
            .context("failed to read pid file")?
            .parse()
            .context("failed to parse pid")?;
        info!("read pid: {:?}", pid);

        self.console = receive_pty_task.join().unwrap();

        self.state = self.state.on_start(state::Start);
        Ok(())
    }

    fn state(&self) -> Result<String> {
        // TODO: check this method for right handle state.
        if self.state == ExecState::exec_created() {
            return Ok(String::from("created"));
        }
        if self.exited != None || self.pid() == 0 {
            Ok(String::from("stopped"))
        } else {
            Ok(String::from("running"))
        }
    }

    fn stdio(&self) -> Stdio {
        self.stdio.clone()
    }

    fn delete(&mut self) -> Result<()> {
        if self.state == ExecState::exec_created() || self.state == ExecState::exec_stopped() {
            // TODO: stop stdin like old shim v1?
            let path = self.path.join(format!("{}.pid", self.id));
            // silently ignore error
            if let Err(e) = std::fs::remove_file(path) {
                info!("cannot remove pid file for {:?}: {:?}", self.id, e);
            }
            self.state = self.state.on_delete(state::Delete);
            Ok(())
        } else if self.state == ExecState::exec_deleted() {
            return Err(ttrpc_error(
                Code::NOT_FOUND,
                format!("cannot delete a {:?} process", self.state),
            )
            .into());
        } else {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot delete a {:?} process", self.state),
            )
            .into());
        }
    }

    fn kill(&self, sig: u32, _all: bool) -> Result<()> {
        if self.state == ExecState::exec_deleted() {
            return Err(ttrpc_error(
                Code::NOT_FOUND,
                format!("cannot kill a {:?} process", self.state),
            )
            .into());
        }
        let pid = self.pid.lock().unwrap();
        if *pid == 0 {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("process not created"),
            )
            .into());
        } else if self.get_exited_at() != None {
            return Err(
                ttrpc_error(Code::NOT_FOUND, String::from("process already finished")).into(),
            );
        } else {
            kill(
                Pid::from_raw(*pid),
                Signal::try_from(sig as libc::c_int).context("failed to get signal")?,
            )
            .context("failed to kill")?;
        }

        Ok(())
    }

    fn set_exited(&mut self, status: i32) {
        if self.state == ExecState::exec_stopped() {
            return;
        }
        let local: chrono::DateTime<chrono::Local> = chrono::Local::now();
        self.status = status;
        self.exited = Some(local);
        self.state = self.state.on_stop(state::Stop);

        let &(ref lock, ref condvar) = &*self.wait_pair;
        let mut exited = lock.lock().unwrap();
        *exited = true;
        condvar.notify_all();
    }

    fn wait(&self) {
        let &(ref lock, ref condvar) = &*self.wait_pair;
        let exited = lock.lock().unwrap();
        let _ = condvar.wait(exited).unwrap();
    }

    fn get_wait_pair(&self) -> Arc<(Mutex<bool>, Condvar)> {
        self.wait_pair.clone()
    }

    fn get_exited_at(&self) -> Option<DateTime<Local>> {
        self.exited
    }

    fn get_exit_status(&self) -> i32 {
        self.status
    }

    fn close_stdin(&mut self) -> Result<()> {
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }

        Ok(())
    }
}

impl RuncProcess for InitProcess {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn pid(&self) -> u32 {
        self.pid as u32
    }

    fn start(&mut self) -> Result<()> {
        if self.state == InitState::created() || self.state == InitState::created_checkpoint() {
            // TODO: checkpoint not finished.
            info!("start runc");
            self.runtime
                .start(&self.id)
                .context("OCI runtime start failed")?;
            self.state = self.state.on_start(state::Start);
            info!("self.state: {:?}", self.state);
            Ok(())
        } else {
            Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot start a {:?} process", self.state),
            )
            .into())
        }
    }

    fn delete(&mut self) -> Result<()> {
        if self.state == InitState::created()
            || self.state == InitState::created_checkpoint()
            || self.state == InitState::stopped()
        {
            self.runtime
                .delete(&self.id, Some(&DeleteOpts { force: false }))
                .context("OCI runtime delete failed")?;
            self.state = self.state.on_delete(state::Delete);

            // TODO: close io
            // Done: umount all
            if self.rootfs != "".to_string() {
                let ret = mount_linux::umount_all(self.rootfs.clone());
                if ret != 0 {
                    return Err(ttrpc_error(
                        Code::FAILED_PRECONDITION,
                        format!(
                            "cannot umount rootfs {:?} for container {:?}",
                            self.rootfs, self.id
                        ),
                    )
                    .into());
                }
            }
            return Ok(());
        } else if self.state == InitState::deleted() {
            return Err(ttrpc_error(
                Code::NOT_FOUND,
                format!("cannot delete a {:?} process", self.state),
            )
            .into());
        } else {
            Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                format!("cannot delete a {:?} process", self.state),
            )
            .into())
        }
    }

    fn kill(&self, sig: u32, all: bool) -> Result<()> {
        if self.state == InitState::deleted() {
            return Err(ttrpc_error(
                Code::NOT_FOUND,
                format!("cannot kill a {:?} process", self.state),
            )
            .into());
        }
        if let Err(e) = self
            .runtime
            .kill(self.id(), sig as i32, Some(&KillOpts { all }))
        {
            match e {
                rust_runc::Error::RuncCommandError { source } => {
                    let msg = source.to_string();
                    let ec = source.raw_os_error();
                    if msg.contains("os: process already finished")
                        || msg.contains("container not running")
                        || msg.to_lowercase().contains("no such process")
                        || ec == Some(Errno::ESRCH as i32)
                    {
                        return Err(ttrpc_error(
                            Code::NOT_FOUND,
                            String::from("process already finished"),
                        )
                        .into());
                    } else if msg.contains("does not exist") {
                        return Err(ttrpc_error(
                            Code::NOT_FOUND,
                            String::from("no such container"),
                        )
                        .into());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn set_exited(&mut self, status: i32) {
        if self.state == InitState::stopped() {
            return;
        }

        let local: chrono::DateTime<chrono::Local> = chrono::Local::now();
        self.status = status;
        self.exited = Some(local);

        if self.state == InitState::paused() {
            // TODO: resume this process
        }

        self.state = self.state.on_stop(state::Stop);

        let &(ref lock, ref condvar) = &*self.wait_pair;
        let mut exited = lock.lock().unwrap();
        *exited = true;
        condvar.notify_all();
    }

    fn wait(&self) {
        let &(ref lock, ref condvar) = &*self.wait_pair;
        let exited = lock.lock().unwrap();
        let _ = condvar.wait(exited).unwrap();
    }

    fn get_wait_pair(&self) -> Arc<(Mutex<bool>, Condvar)> {
        self.wait_pair.clone()
    }

    fn state(&self) -> Result<String> {
        //TODO: no have pausing state
        if self.state == InitState::created() || self.state == InitState::created_checkpoint() {
            Ok(String::from("created"))
        } else if self.state == InitState::running() {
            Ok(String::from("running"))
        } else if self.state == InitState::paused() {
            Ok(String::from("paused"))
        } else if self.state == InitState::stopped() || self.state == InitState::deleted() {
            Ok(String::from("stopped"))
        } else {
            Err(ShimError::AnyhowError(anyhow!("Status Unknown")))
        }
    }

    fn stdio(&self) -> Stdio {
        self.stdio.clone()
    }

    fn get_exited_at(&self) -> Option<DateTime<Local>> {
        self.exited
    }

    fn container_id(&self) -> &str {
        &self.container_id
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn resize(&self, ws: &WinSize) {
        let ws = nix::pty::Winsize {
            ws_row: ws.height,
            ws_col: ws.width,
            ws_xpixel: ws.x,
            ws_ypixel: ws.y,
        };

        if let Some(console) = self.console {
            unsafe { set_term_size_unsafe(console, &ws as *const nix::pty::Winsize) }
                .unwrap_or_else(|e| {
                    info!("set term size failed: {:?}", e);
                    0
                });
        }
    }

    fn get_exit_status(&self) -> i32 {
        self.status
    }

    fn close_stdin(&mut self) -> Result<()> {
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }

        Ok(())
    }
}
