use crate::process::InitProcess;
use crate::shim_service::{set_proto_timestamp, Shared};
use anyhow::{Context, Result};
use log::{debug, info};
use nix::errno::{errno, Errno};
use nix::sys::signal::{
    pthread_sigmask, sigaction, SaFlags, SigAction, SigHandler, SigSet, SigmaskHow, Signal,
};
use nix::{
    sys::wait::{waitpid, WaitPidFlag, WaitStatus},
    unistd::Pid,
};
use oci_spec::runtime::{LinuxNamespaceType, Spec};
use protobuf::Message;
use protocols::events;
use rust_runc::ExitInfo;
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

pub struct Publisher {
    containerd_binary: String,
    address: String,
    namespace: String,
}

impl Publisher {
    pub fn new(binary: &str, address: &str, namespace: &str) -> Self {
        Publisher {
            containerd_binary: String::from(binary),
            address: String::from(address),
            namespace: String::from(namespace),
        }
    }

    pub fn publish<M: Message>(&self, topic: &str, message: M) -> Result<()> {
        let any =
            protobuf::well_known_types::Any::pack(&message).context("failed to pack message")?;

        let mut any_clone = any.clone();
        let type_url: Vec<&str> = any.get_type_url().split("/").collect();
        any_clone.set_type_url(String::from(type_url[1]));

        let mut child = std::process::Command::new(&self.containerd_binary)
            .arg("--address")
            .arg(&self.address)
            .arg("publish")
            .arg("--topic")
            .arg(topic)
            .arg("--namespace")
            .arg(&self.namespace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to publish")?;
        let sin = child.stdin.as_mut().context("failed to open stdin")?;
        any_clone.write_to_writer(sin).context("failed to write")?;
        Ok(())
    }
}

pub struct Trap {
    oldset: SigSet,
    oldsigs: Vec<(Signal, SigAction)>,
    sigset: SigSet,
}

extern "C" fn empty_handler(_: libc::c_int) {}

impl Trap {
    /// Create and activate the signal trap for specified signals. Signals not
    /// in list will be delivered asynchronously as always.
    pub fn trap(signals: &[Signal]) -> Trap {
        unsafe {
            let mut sigset = SigSet::empty();
            for &sig in signals {
                sigset.add(sig);
            }
            let mut oldset = SigSet::empty();
            let mut oldsigs = Vec::new();
            pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&sigset), Some(&mut oldset)).unwrap();
            // Set signal handlers to an empty function, this allows ignored
            // signals to become pending, effectively allowing them to be
            // waited for.
            for &sig in signals {
                oldsigs.push((
                    sig,
                    sigaction(
                        sig,
                        &SigAction::new(
                            SigHandler::Handler(empty_handler),
                            SaFlags::SA_SIGINFO,
                            sigset,
                        ),
                    )
                    .unwrap(),
                ));
            }
            Trap {
                oldset,
                oldsigs,
                sigset,
            }
        }
    }
}

impl Iterator for Trap {
    type Item = libc::siginfo_t;
    fn next(&mut self) -> Option<libc::siginfo_t> {
        let info = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
        let mut info = unsafe { info.assume_init() };
        //let mut info: libc::siginfo_t = unsafe { std::mem::uninitialized() };
        loop {
            if unsafe { libc::sigwaitinfo(self.sigset.as_ref(), &mut info) } != -1 {
                return Some(info);
            } else {
                if Errno::last() == Errno::EINTR {
                    continue;
                }
                panic!("Sigwait error: {}", errno());
            }
        }
    }
}

impl Drop for Trap {
    fn drop(&mut self) {
        unsafe {
            for &(sig, ref sigact) in self.oldsigs.iter() {
                sigaction(sig, sigact).unwrap();
            }
            pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&self.oldset), None).unwrap();
        }
    }
}

impl fmt::Debug for Trap {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Trap").finish()
    }
}

pub struct Reaper {
    service: Arc<Shared>,
    publisher: Arc<Publisher>,
    trap: Trap,
    exits: Arc<ExitInfo>,
}

impl Reaper {
    pub fn new(
        s: Arc<Shared>,
        publisher: Publisher,
        signals: &[Signal],
        exits: &Arc<ExitInfo>,
    ) -> Self {
        Reaper {
            service: s,
            publisher: Arc::new(publisher),
            trap: Trap::trap(signals),
            exits: Arc::clone(exits),
        }
    }

    pub fn handle_signals(&mut self) {
        while let Some(info) = self.trap.next() {
            let signal = info.si_signo;
            info!("event received, signal: {:?}", signal);

            match signal {
                libc::SIGCHLD => loop {
                    match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
                        Ok(status) => {
                            let (pid, exit_code) = match status {
                                WaitStatus::StillAlive => break,
                                WaitStatus::Exited(p, ec) => {
                                    info!("pid: {:?} exited with: {}", p, ec);
                                    (p, ec)
                                }
                                WaitStatus::Signaled(p, signal, _) => {
                                    info!("pid: {:?} exit signal: {:?}", p, signal);
                                    (p, signal as i32 + 128)
                                }
                                _ => {
                                    info!("wait status: {:?}", status);
                                    break;
                                }
                            };
                            {
                                let mut list = self.exits.list.lock().unwrap();
                                list.insert(pid, exit_code);
                            }

                            if let Err(e) =
                                Self::check_process(&self.service, &self.publisher, pid, exit_code)
                            {
                                info!("publish events failed: {:?}", e);
                            }
                        }
                        Err(e) => {
                            info!("waitpid failed: {}", e);
                            if e.as_errno() == Some(Errno::ECHILD) {
                                break;
                            }
                        }
                    }
                },
                libc::SIGPIPE => {}
                _ => info!("unhandle signal: {}", signal),
            }
        }
    }

    fn should_kill_all_on_exit(bundle: &str) -> bool {
        let path = PathBuf::from(bundle).join("config.json");
        let spec: Spec = match Spec::load(path.to_string_lossy().as_ref()) {
            Ok(s) => s,
            Err(e) => {
                info!("should_kill_all_on_exit: failed to load config.json: {}", e);
                return true;
            }
        };

        if let Some(linux) = spec.linux {
            if let Some(namespaces) = linux.namespaces {
                for ns in namespaces {
                    match ns.typ {
                        LinuxNamespaceType::pid => {
                            if ns.path.map_or_else(|| true, |v| v == "") {
                                return false;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        true
    }

    fn check_process(service: &Shared, publisher: &Publisher, pid: Pid, status: i32) -> Result<()> {
        let mut s = service.inner.lock().unwrap();
        let ref mut processes = s.processes;
        for (id, process) in processes.iter_mut() {
            info!("check process exit: {}", id);
            if process.pid() == (pid.as_raw() as u32) {
                info!("process id: {}", process.pid());
                match process.as_any().downcast_ref::<InitProcess>() {
                    Some(init) => {
                        let ref bundle = init.bundle;
                        if Self::should_kill_all_on_exit(bundle) {
                            debug!("should kill all on exit.");
                            init.kill_all()?;
                        }
                    }
                    None => {}
                };

                process.set_exited(status);
                let mut timestamp = protobuf::well_known_types::Timestamp::new();
                if let Some(exited_at) = process.get_exited_at() {
                    set_proto_timestamp(&mut timestamp, &exited_at);
                }
                let ts = protobuf::SingularPtrField::some(timestamp);
                let event = events::TaskExit {
                    container_id: String::from(process.container_id()),
                    id: String::from(process.id()),
                    pid: pid.as_raw() as u32,
                    exit_status: status as u32,
                    exited_at: ts,
                    ..Default::default()
                };

                debug!("publish event: {:?}", event);
                publisher.publish("/tasks/exit", event)?;
            }
        }
        Ok(())
    }
}
