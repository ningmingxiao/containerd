/*
   Copyright The containerd Authors.

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
*/

use std::os::unix::io::RawFd;
use std::path::Path;
use std::sync::Arc;

use log::{debug, warn};
use nix::cmsg_space;
use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
use nix::sys::termios::tcgetattr;
use nix::sys::uio::IoVec;
use oci_spec::runtime::{LinuxNamespaceType, Spec};

use containerd_shim::api::{ExecProcessRequest, Options};
use containerd_shim::io::Stdio;
use containerd_shim::{io_error, other, other_error, Error};
use runc::io::{Io, NullIo, PipedIo, IOOption, BinaryIO};
use runc::options::GlobalOpts;
use runc::{Runc, Spawner};
use url::Url;

pub const GROUP_LABELS: [&str; 2] = [
    "io.containerd.runc.v2.group",
    "io.kubernetes.cri.sandbox-id",
];
pub const INIT_PID_FILE: &str = "init.pid";

pub struct ProcessIO {
    pub uri: Option<String>,
    pub io: Option<Arc<dyn Io>>,
    pub copy: bool,
    pub stdio_path: Stdio,
}

pub fn create_io(
    id: &str,
    io_uid: u32,
    io_gid: u32,
    stdio: &Stdio,
    ns: &str,
) -> containerd_shim::Result<ProcessIO> {
    if stdio.is_null() {
        let nio = NullIo::new().map_err(io_error!(e, "new Null Io"))?;
        let pio = ProcessIO {
            uri: None,
            io: Some(Arc::new(nio)),
            copy: false,
            stdio_path: stdio.clone(),
        };
        return Ok(pio);
    }
    let stdout = stdio.stdout.as_str();
    let scheme_path = stdout.trim().split("://").collect::<Vec<&str>>();
    let scheme: &str;
    let uri: String;
    let std_url: Url;
    if scheme_path.len() <= 1 {
        // no scheme specified
        // default schema to fifo
        uri = format!("fifo://{}", stdout);
        scheme = "fifo";
        std_url = Url::parse((scheme.to_owned() + "://" + stdio.stdout.as_str()).as_str())?;
    } else {
        uri = stdout.to_string();
        std_url = Url::parse(stdio.stdout.as_str())?;
        scheme = std_url.scheme();
    }

    let mut pio = ProcessIO {
        uri: Some(uri),
        io: None,
        copy: false,
        stdio_path: Stdio{
            stdin: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            terminal: stdio.terminal
        },
    };

    if scheme == "binary" {
        let mut io = match BinaryIO::new(id, &std_url, ns) {
            Ok(io) => {
                io
            }
            Err(e) => {
                return Err(Error::IoError { context: "create BinaryIO err".to_string(), err: e })
            }
        };
        unsafe {
            io.create_binary_io().expect("binary IO create failed");
        }
        pio.io = Some(Arc::new(io));
    }

    else if scheme == "fifo" || scheme == "file" {
        debug!(
            "create named pipe io for container {}, stdin: {}, stdout: {}, stderr: {}",
            id,
            stdio.stdin.as_str(),
            stdio.stdout.as_str(),
            stdio.stderr.as_str()
        );

        let opts = IOOption {
            open_stdin: !stdio.stdin.is_empty(),
            open_stdout: !stdio.stdout.is_empty(),
            open_stderr: !stdio.stderr.is_empty(),
        };
        let io = PipedIo::new(io_uid, io_gid, &opts).unwrap();
        set_io(&scheme_path, &mut pio, stdio, &opts)?;

        pio.io = Some(Arc::new(io));
        pio.copy = true;
    }

    else {
        return Err(Error::Other("unknown scheme".to_string()));
    }

    Ok(pio)
}

#[derive(Default, Debug)]
pub struct ShimExecutor {}

pub fn get_spec_from_request(
    req: &ExecProcessRequest,
) -> containerd_shim::Result<oci_spec::runtime::Process> {
    if let Some(val) = req.spec.as_ref() {
        let mut p = serde_json::from_slice::<oci_spec::runtime::Process>(val.get_value())?;
        p.set_terminal(Some(req.terminal));
        Ok(p)
    } else {
        Err(Error::InvalidArgument("no spec in request".to_string()))
    }
}

pub fn check_kill_error(emsg: String) -> Error {
    let emsg = emsg.to_lowercase();
    if emsg.contains("process already finished")
        || emsg.contains("container not running")
        || emsg.contains("no such process")
    {
        Error::NotFoundError("process already finished".to_string())
    } else if emsg.contains("does not exist") {
        Error::NotFoundError("no such container".to_string())
    } else {
        other!("unknown error after kill {}", emsg)
    }
}

const DEFAULT_RUNC_ROOT: &str = "/run/containerd/runc";
const DEFAULT_COMMAND: &str = "runc";

pub fn create_runc(
    runtime: &str,
    namespace: &str,
    bundle: impl AsRef<Path>,
    opts: &Options,
    spawner: Option<Arc<dyn Spawner + Send + Sync>>,
) -> containerd_shim::Result<Runc> {
    let runtime = if runtime.is_empty() {
        DEFAULT_COMMAND
    } else {
        runtime
    };
    let root = opts.root.as_str();
    let root = Path::new(if root.is_empty() {
        DEFAULT_RUNC_ROOT
    } else {
        root
    })
    .join(namespace);

    let log = bundle.as_ref().join("log.json");
    let mut gopts = GlobalOpts::default()
        .command(runtime)
        .root(root)
        .log(log)
        .log_json()
        .systemd_cgroup(opts.systemd_cgroup);
    if let Some(s) = spawner {
        gopts.custom_spawner(s);
    }
    gopts
        .build()
        .map_err(other_error!(e, "unable to create runc instance"))
}

#[derive(Default)]
pub(crate) struct CreateConfig {}

pub fn receive_socket(stream_fd: RawFd) -> containerd_shim::Result<RawFd> {
    let mut buf = [0u8; 4096];
    let iovec = [IoVec::from_mut_slice(&mut buf)];
    let mut space = cmsg_space!([RawFd; 2]);
    let (path, fds) = match recvmsg(stream_fd, &iovec, Some(&mut space), MsgFlags::empty()) {
        Ok(msg) => {
            let mut iter = msg.cmsgs();
            if let Some(ControlMessageOwned::ScmRights(fds)) = iter.next() {
                (iovec[0].as_slice(), fds)
            } else {
                return Err(other!("received message is empty"));
            }
        }
        Err(e) => {
            return Err(other!("failed to receive message: {}", e));
        }
    };
    if fds.is_empty() {
        return Err(other!("received message is empty"));
    }
    let path = String::from_utf8(Vec::from(path)).unwrap_or_else(|e| {
        warn!("failed to get path from array {}", e);
        "".to_string()
    });
    let path = path.trim_matches(char::from(0));
    debug!(
        "copy_console: console socket get path: {}, fd: {}",
        path, &fds[0]
    );
    tcgetattr(fds[0])?;
    Ok(fds[0])
}

pub fn has_shared_pid_namespace(spec: &Spec) -> bool {
    match spec.linux() {
        None => true,
        Some(linux) => match linux.namespaces() {
            None => true,
            Some(namespaces) => {
                for ns in namespaces {
                    if ns.typ() == LinuxNamespaceType::Pid && ns.path().is_none() {
                        return false;
                    }
                }
                true
            }
        },
    }
}

pub fn set_io(scheme_path: &Vec<&str>, pio: &mut ProcessIO, stdio: &Stdio, opts: &IOOption) -> containerd_shim::Result<()> {
    if scheme_path.len() <= 1 {
        // no scheme specified
        // default schema to fifo
        if opts.open_stdin {
            pio.stdio_path.stdin = stdio.clone().stdin
        }
        pio.stdio_path.stdout = stdio.clone().stdout;
        pio.stdio_path.stderr = stdio.clone().stderr;
        debug!("pio.stdio_path.stdout = {}", pio.stdio_path.stdout);
    } else {
        if opts.open_stdin {
            pio.stdio_path.stdin = Url::parse(stdio.stdin.as_str())?
                .path()
                .to_string()
        }
        pio.stdio_path.stdout = Url::parse(stdio.stdout.as_str())?
            .path()
            .to_string();
        pio.stdio_path.stderr = Url::parse(stdio.stderr.as_str())?
            .path()
            .to_string();
    }
    Ok(())
}
