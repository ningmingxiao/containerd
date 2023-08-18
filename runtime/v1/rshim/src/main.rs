use anyhow::{Context, Result};
use backtrace::Backtrace;
use chrono::DateTime;
use log::{error, info, LevelFilter};
use nix::sys::signal::Signal;
use rust_runc::ExitInfo;
use std::collections::HashMap;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::sync::Mutex;
use std::{env, path::PathBuf};
mod args;
mod console;
mod errors;
mod mount_linux;
mod process;
mod reaper;
mod shim_service;
mod state;
mod types;

struct Logger {
    output: Mutex<Box<dyn std::io::Write + Send>>,
    start: std::time::Instant,
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let now = std::time::Instant::now();
        let _ = now.duration_since(self.start);
        let now: DateTime<chrono::Local> = chrono::Local::now();

        if record.file().is_some() && record.line().is_some() {
            writeln!(
                *(*(self.output.lock().unwrap())),
                "{} [{}] <{}:{}> {}",
                now.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
                record.level(),
                record.file().unwrap(),
                record.line().unwrap(),
                record.args()
            )
        } else {
            writeln!(
                *(*(self.output.lock().unwrap())),
                "{} [{}] <{}> {}",
                now.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
                record.level(),
                record.target(),
                record.args()
            )
        }
        .ok();
    }
    fn flush(&self) {}
}

fn open_stdio_keep_alive_pipes(workdir: &str) -> Result<(File, File)> {
    let dir = PathBuf::from(workdir);
    let stdout = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&dir.join("shim.stdout.log"))?;

    let stderr = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&dir.join("shim.stderr.log"))?;

    Ok((stdout, stderr))
}

fn parse_config(arguments: &args::Arguments) -> Result<shim_service::ShimConfig> {
    let work_dir = arguments
        .value_of("workdir")
        .unwrap_or(env::temp_dir().to_str().context("failed to get temp_dir")?)
        .to_string();

    let mut log_level = LevelFilter::Info;
    if arguments.is_present("debug") {
        log_level = LevelFilter::Debug;
    }

    let (stdout, _) = open_stdio_keep_alive_pipes(&work_dir)?;
    log::set_boxed_logger(Box::new(Logger {
        output: Mutex::new(Box::new(stdout)),
        start: std::time::Instant::now(),
    }))
    .map(|()| log::set_max_level(log_level))
    .context("expected to be able to setup logger")?;

    let namespace = arguments
        .value_of("namespace")
        .unwrap_or("moby")
        .to_string();
    let criu = arguments.value_of("criu").unwrap_or("").to_string();
    let runtime_root = arguments.value_of("runtime-root").unwrap_or("").to_string();
    let systemd_cgroup = arguments.is_present("systemd-cgroup");

    info!(
        "namespace: {}, work_dir: {}, criu: {}, runtime-root: {}, systemd-cgroup: {}",
        namespace, work_dir, criu, runtime_root, systemd_cgroup
    );

    Ok(shim_service::ShimConfig {
        path: env::current_dir().unwrap().to_string_lossy().to_string(),
        namespace,
        work_dir,
        criu,
        runtime_root,
        systemd_cgroup,
    })
}

fn main() {

    let mut arguments = args::Arguments::new();
    arguments.parse(env::args());

    let config = match parse_config(&arguments) {
        Ok(c) => c,
        Err(e) => {
            panic!("failed to parse config arguments: {:?}", e);
        }
    };

    let vec : Vec<&str> = Box::leak(config.work_dir.clone().into_boxed_str()).split("k8s.io/").collect();

    std::panic::set_hook(Box::new(move |panic_info| {
        let info = match panic_info.payload().downcast_ref::<&'static str>() {
            Some(s) => *s,
            None => match panic_info.payload().downcast_ref::<String>() {
                Some(s) => &s[..],
                None => "Box<dyn Any>",
            },
        };

        let pid = unsafe { libc::getpid() };
        let location = panic_info.location().unwrap();
        let panic_msg = format!("process '{}' panicked at '{}', '{}'\n\nstack backtrace:\n{:?}", pid, info, location, Backtrace::new());

        let dir  = "/var/log/rshim/";

        match fs::create_dir_all(dir) {
            Ok(_) => {},
            Err(e) => {
                error!("failed to create dir {:?} with {:?}", dir, e);
                std::process::exit(1);
            },

        };
        let file_name = dir.to_string() + vec[1] + ".txt";
    
        let mut backtrace_file = match File::create(file_name.clone()) {
            Ok(file) => file,
            Err(e) => {
                error!("failed to create file {:?} with {:?}", file_name.clone(), e);
                std::process::exit(1);
            },
        };
        match backtrace_file.write_all(panic_msg.as_bytes()) {
            Ok(_) => {},
            Err(e) => {
                error!("failed to write file {:?} with {:?}", file_name.clone(), e);
                std::process::exit(1);
            },
        };

        std::process::exit(1);
    }));


    let containerd_binary = arguments
        .value_of("containerd-binary")
        .unwrap_or("containerd");
    let address = arguments.value_of("address").unwrap_or("");
    let socket = arguments.value_of("socket").unwrap_or("");
    let namespace = arguments.value_of("namespace").unwrap_or("moby");

    match prctl::set_child_subreaper(true) {
        Ok(_) => {}
        Err(e) => {
            error!("failed to set subscreaper: {:?}", e);
        }
    }

    let exits = Arc::new(ExitInfo {
        list: Mutex::new(HashMap::new()),
    });

    let shim_service = shim_service::ShimService::new(config, &exits);
    let service = Arc::clone(&shim_service.service);
    let mut reaper = reaper::Reaper::new(
        service,
        reaper::Publisher::new(containerd_binary, address, namespace),
        &[Signal::SIGPIPE, Signal::SIGCHLD],
        &exits,
    );

    match shim_service::create_server(shim_service, socket) {
        Ok(s) => {
            let mut s = s.set_thread_count_min(1);
            s = s.set_thread_count_default(2);
            s = s.set_thread_count_max(3);
            match s.start() {
                Ok(_) => {}
                Err(e) => {
                    error!("failed to start shim service: {:?}", e);
                }
            };
        }
        Err(e) => {
            error!("create server failed: {:?}", e);
        }
    }

    reaper.handle_signals();
}
