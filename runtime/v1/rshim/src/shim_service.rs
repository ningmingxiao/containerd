#![allow(non_snake_case)]
#![allow(unused_variables)]
use crate::errors::{ttrpc_error, Result, ShimError};
use crate::mount_linux;
use crate::process::{InitProcess, RuncProcess, WinSize};
use crate::types;
use anyhow::anyhow;
use chrono::{DateTime, Local};
use log::{error, info};
use protobuf::well_known_types::Timestamp;
use protocols::{empty, runc, shim, shim_ttrpc, task};
use rust_runc::specs::LinuxResources;
use rust_runc::ExitInfo;
use std::collections::HashMap;
use std::convert::From;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use ttrpc::{server, Code};

pub struct ShimConfig {
    pub path: String,
    pub namespace: String,
    pub work_dir: String,
    pub criu: String,
    pub runtime_root: String,
    pub systemd_cgroup: bool,
}

pub struct Inner {
    pub processes: HashMap<String, Box<dyn RuncProcess + Send + Sync>>,
    pub id: Option<String>,
    pub bundle: Option<String>,
}

pub struct Shared {
    pub inner: Mutex<Inner>,
}

pub struct ShimService {
    pub config: ShimConfig,
    pub service: Arc<Shared>,
    pub exits: Arc<ExitInfo>,
}

pub fn create_server(service: ShimService, path: &str) -> Result<server::Server> {
    let shim = Box::new(service) as Box<dyn shim_ttrpc::Shim + Send + Sync>;
    let worker = Arc::new(shim);
    let shim_service = shim_ttrpc::create_shim(worker);

    let server: ttrpc::Server;
    if path == "" {
        server = server::Server::new()
            .add_listener(3)?
            .register_service(shim_service);
        info!("serving api on unix socket: [inherited from parent]");
    } else {
        if path.len() > 106 {
            return Err(ShimError::AnyhowError(anyhow!(
                "{}: unix socket path too long (> 106)",
                path
            )));
        }
        let host = ["unix://", path].join("");
        server = server::Server::new().bind(&host)?;
        info!("serving api on unix socket: {}", path);
    }

    Ok(server)
}

pub fn set_proto_timestamp(timestamp: &mut Timestamp, exited_at: &DateTime<Local>) {
    let exited_at_timestamp = exited_at.timestamp();
    let exited_at_nanos: i32 = (exited_at.timestamp_nanos() % (1e9 as i64)) as i32;
    timestamp.set_seconds(exited_at_timestamp);
    timestamp.set_nanos(exited_at_nanos);
}

impl ShimService {
    pub fn new(config: ShimConfig, exits: &Arc<ExitInfo>) -> Self {
        ShimService {
            config,
            service: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    processes: HashMap::new(),
                    id: None,
                    bundle: None,
                }),
            }),
            exits: Arc::clone(&exits),
        }
    }

    pub fn get_init_process_as_mut<'a>(
        &self,
        processes: &'a mut HashMap<String, Box<dyn RuncProcess + Send + Sync>>,
        sid: &str,
    ) -> Result<&'a mut InitProcess> {
        match processes.get_mut(sid) {
            Some(process) => match process.as_mut_any().downcast_mut::<InitProcess>() {
                Some(init) => Ok(init),
                None => Err(ttrpc_error(
                    Code::INTERNAL,
                    String::from("cast process to InitProcess failed."),
                )
                .into()),
            },
            None => Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("container must be created"),
            )
            .into()),
        }
    }

    pub fn get_init_process<'a>(
        &self,
        processes: &'a HashMap<String, Box<dyn RuncProcess + Send + Sync>>,
        sid: &str,
    ) -> Result<&'a InitProcess> {
        match processes.get(sid) {
            Some(process) => match process.as_any().downcast_ref::<InitProcess>() {
                Some(init) => Ok(init),
                None => Err(ttrpc_error(
                    Code::INTERNAL,
                    String::from("cast process to InitProcess failed."),
                )
                .into()),
            },
            None => Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("container must be created"),
            )
            .into()),
        }
    }
}

impl shim_ttrpc::Shim for ShimService {
    fn state(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::StateRequest,
    ) -> ttrpc::Result<shim::StateResponse> {
        info!("state: {:?}", req);
        let s = self.service.inner.lock().unwrap();
        if let Some(p) = s.processes.get(&req.id) {
            let status = match p.state()?.as_ref() {
                "created" => task::Status::CREATED,
                "running" => task::Status::RUNNING,
                "stopped" => task::Status::STOPPED,
                "paused" => task::Status::PAUSED,
                "pausing" => task::Status::PAUSING,
                _ => task::Status::UNKNOWN,
            };
            info!("status: {:?}", status);
            let sio = p.stdio();
            let bundle = if let Some(ref b) = s.bundle {
                b.clone()
            } else {
                String::from("")
            };

            if let Some(ref id) = s.id {
                let mut timestamp = Timestamp::new();
                if let Some(exited_at) = p.get_exited_at() {
                    set_proto_timestamp(&mut timestamp, &exited_at);
                }
                let ts = protobuf::SingularPtrField::some(timestamp);
                return Ok(shim::StateResponse {
                    id: p.id().to_string(),
                    bundle,
                    pid: p.pid(),
                    status,
                    stdin: sio.stdin,
                    stdout: sio.stdout,
                    stderr: sio.stderr,
                    terminal: sio.terminal,
                    exit_status: p.get_exit_status() as u32,
                    exited_at: ts,
                    ..Default::default()
                });
            }
        }

        Err(ttrpc_error(Code::NOT_FOUND, format!("id: {}", req.id)))
    }

    fn create(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::CreateTaskRequest,
    ) -> ttrpc::Result<shim::CreateTaskResponse> {
        info!(
            "req.stdin = {}, req.stdout = {}, req.stderr = {}",
            req.stdin, req.stdout, req.stderr
        );
        info!("create: {:?}", req);
        let mut type_mounts = Vec::<types::Mount>::new();

        for m in req.rootfs.to_vec() {
            let mount = types::Mount::new(m.field_type, m.source, m.target, m.options.to_vec());
            type_mounts.push(mount);
        }

        let path = PathBuf::from(req.bundle.clone()).join("rootfs");

        let rootfs = match path.to_str() {
            Some(p) => p,
            None => {
                return Err(ttrpc_error(
                    Code::INTERNAL,
                    String::from("rootfs is not valid"),
                ))
            }
        };

        let mut ret;
        for tm in type_mounts {
            let m = mount_linux::Mount::new(tm.Type, tm.Source, tm.Options);
            ret = m.mount(rootfs.to_string());
            if ret != 0 {
                error!("failed to mount rootfs component {:?}", m);
                let err_msg = format!("failed to mount rootfs component {:?}", m);
                ret = mount_linux::umount_all(rootfs.to_string());
                if ret != 0 {
                    error!("failed to cleanup rootfs mount");
                }
                return Err(ttrpc_error(Code::INTERNAL, err_msg));
            }
        }

        let mut process = match InitProcess::new(
            &self.config.path,
            &self.config.work_dir,
            &self.config.runtime_root,
            &self.config.namespace,
            &self.config.criu,
            self.config.systemd_cgroup,
            &req,
            rootfs,
            &self.exits,
        ) {
            Ok(p) => p,
            Err(e) => {
                ret = mount_linux::umount_all(rootfs.to_string());
                if ret != 0 {
                    error!("failed to cleanup rootfs mount");
                }
                return Err(e)?;
            }
        };

        match process.create() {
            Ok(_) => {
                let pid = process.pid as u32;
                let mut s = self.service.inner.lock().unwrap();
                s.processes.insert(process.id.clone(), Box::new(process));
                s.id = Some(req.id);
                s.bundle = Some(req.bundle);
                Ok(shim::CreateTaskResponse {
                    pid,
                    ..Default::default()
                })
            }
            Err(e) => {
                ret = mount_linux::umount_all(rootfs.to_string());
                if ret != 0 {
                    error!("failed to cleanup rootfs mount");
                }
                Err(e)?
            }
        }
    }

    fn start(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::StartRequest,
    ) -> ttrpc::Result<shim::StartResponse> {
        info!("start: {:?}", req);
        let mut s = self.service.inner.lock().unwrap();
        match s.processes.get_mut(&req.id) {
            Some(p) => {
                info!("start process {}", p.pid());
                p.start()?;
                Ok(shim::StartResponse {
                    id: req.id,
                    pid: p.pid(),
                    ..Default::default()
                })
            }
            None => {
                info!("container not created: {}", &req.id);
                Err(ttrpc_error(
                    Code::NOT_FOUND,
                    String::from("create runc failed"),
                ))
            }
        }
    }

    fn delete(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        _req: empty::Empty,
    ) -> ttrpc::Result<shim::DeleteResponse> {
        info!("delete");
        let mut s = self.service.inner.lock().unwrap();
        if s.id.is_some() {
            let sid = (match s.id.as_ref() {
                Some(id) => id,
                None => {
                    return Err(ttrpc_error(
                        Code::INTERNAL,
                        String::from("failed to get id in delete"),
                    ))
                }
            })
            .clone();
            if let Some(p) = s.processes.get_mut(&sid) {
                p.delete()?;

                let mut timestamp = Timestamp::new();
                if let Some(exited_at) = p.get_exited_at() {
                    set_proto_timestamp(&mut timestamp, &exited_at);
                }
                let ts = protobuf::SingularPtrField::some(timestamp);
                let pid = p.pid();
                let exit_status = p.get_exit_status() as u32;
                s.processes.remove(&sid);

                return Ok(shim::DeleteResponse {
                    pid,
                    exit_status,
                    exited_at: ts,
                    ..Default::default()
                });
            }
        }

        Err(ttrpc_error(
            Code::NOT_FOUND,
            String::from("container must be created"),
        ))
    }

    fn delete_process(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::DeleteProcessRequest,
    ) -> ttrpc::Result<shim::DeleteResponse> {
        info!("delete process");
        let mut s = self.service.inner.lock().unwrap();
        if s.id.is_none() {
            return Err(ttrpc_error(Code::NOT_FOUND, format!("id: {}", req.id)));
        }

        let sid = match s.id.as_ref() {
            Some(id) => id,
            None => {
                return Err(ttrpc_error(
                    Code::INTERNAL,
                    String::from("failed to get id in delete_process"),
                ))
            }
        };
        if req.id.eq(sid) {
            return Err(ttrpc_error(
                Code::INVALID_ARGUMENT,
                String::from("cannot delete init process with DeleteProcess"),
            ));
        }

        if let Some(p) = s.processes.get_mut(&req.id) {
            let pid = p.pid();
            info!("delete process {}", pid);
            p.delete()?;
            let mut timestamp = Timestamp::new();
            if let Some(exited_at) = p.get_exited_at() {
                set_proto_timestamp(&mut timestamp, &exited_at);
            }
            let ts = protobuf::SingularPtrField::some(timestamp);
            let exit_status = p.get_exit_status() as u32;
            s.processes.remove(&req.id);
            return Ok(shim::DeleteResponse {
                pid,
                exit_status,
                exited_at: ts,
                ..Default::default()
            });
        }
        info!("process not created: {}", &req.id);
        Err(ttrpc_error(
            Code::NOT_FOUND,
            String::from("create runc failed"),
        ))
    }

    fn list_pids(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        _req: shim::ListPidsRequest,
    ) -> ttrpc::Result<shim::ListPidsResponse> {
        let s = self.service.inner.lock().unwrap();
        if s.id.is_none() {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("container must be created"),
            ));
        }

        let init = self.get_init_process(
            &s.processes,
            match s.id.as_ref() {
                Some(id) => id,
                None => {
                    return Err(ttrpc_error(
                        Code::INTERNAL,
                        String::from("failed to get id in list_pids"),
                    ))
                }
            },
        )?;
        let mut processes = protobuf::RepeatedField::new();
        let pids = init.ps()?;
        for pid in pids {
            let mut info = task::ProcessInfo {
                pid: pid as u32,
                ..Default::default()
            };
            for process in s.processes.values() {
                if pid as u32 == process.pid() {
                    let detail = runc::ProcessDetails {
                        exec_id: String::from(process.id()),
                        ..Default::default()
                    };
                    let any = match protobuf::well_known_types::Any::pack(&detail) {
                        Ok(a) => a,
                        Err(e) => {
                            return Err(ttrpc_error(
                                Code::INTERNAL,
                                String::from("failed to pack detail"),
                            ))
                        }
                    };
                    let mut any_clone = any.clone();
                    let type_url: Vec<&str> = any.get_type_url().split("/").collect();
                    any_clone.set_type_url(String::from(type_url[1]));
                    info.set_info(any_clone);
                }
            }
            processes.push(info);
        }

        Ok(shim::ListPidsResponse {
            processes,
            ..Default::default()
        })
    }

    fn pause(&self, _ctx: &ttrpc::TtrpcContext, _req: empty::Empty) -> ttrpc::Result<empty::Empty> {
        info!("pause");
        let mut s = self.service.inner.lock().unwrap();
        if s.id.is_none() {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("container must be created"),
            ));
        }

        let sid = (match s.id.as_ref() {
            Some(id) => id,
            None => {
                return Err(ttrpc_error(
                    Code::INTERNAL,
                    String::from("failed to get id in pause"),
                ))
            }
        })
        .clone();
        let init = self.get_init_process_as_mut(&mut s.processes, &sid)?;
        init.pause()?;
        Ok(empty::Empty::new())
    }

    fn resume(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        _req: empty::Empty,
    ) -> ttrpc::Result<empty::Empty> {
        info!("resume");
        let mut s = self.service.inner.lock().unwrap();
        if s.id.is_none() {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("container must be created"),
            ));
        }
        let sid = (match s.id.as_ref() {
            Some(id) => id,
            None => {
                return Err(ttrpc_error(
                    Code::INTERNAL,
                    String::from("failed to get id in resume"),
                ))
            }
        })
        .clone();
        let init = self.get_init_process_as_mut(&mut s.processes, &sid)?;
        init.resume()?;
        Ok(empty::Empty::new())
    }

    fn checkpoint(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        _req: shim::CheckpointTaskRequest,
    ) -> ttrpc::Result<empty::Empty> {
        Err(ttrpc_error(
            Code::NOT_FOUND,
            String::from("/containerd.runtime.linux.shim.v1.Shim/Checkpoint is not supported"),
        ))
    }

    fn kill(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::KillRequest,
    ) -> ttrpc::Result<empty::Empty> {
        info!("kill: {:?}", req);
        let s = self.service.inner.lock().unwrap();
        let sid = if req.id == "" {
            s.id.as_ref().unwrap_or(&req.id)
        } else {
            &req.id
        };

        match s.processes.get(sid) {
            Some(p) => {
                info!("kill process {}", p.pid());
                p.kill(req.signal, req.all)?;
                Ok(empty::Empty::new())
            }
            None => {
                info!("container not kill: {}", &req.id);
                Err(ttrpc_error(
                    Code::NOT_FOUND,
                    String::from("container not found."),
                ))
            }
        }
    }

    fn exec(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::ExecProcessRequest,
    ) -> ttrpc::Result<empty::Empty> {
        info!("exec: {:?}", req);
        let mut s = self.service.inner.lock().unwrap();
        if s.processes.get(&req.id).is_some() {
            info!("exec: already exists");
            return Err(ttrpc_error(Code::ALREADY_EXISTS, format!("id: {}", req.id)));
        }

        if s.id.is_none() {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("container must be created"),
            ));
        }

        let init = self.get_init_process(
            &s.processes,
            match s.id.as_ref() {
                Some(id) => id,
                None => {
                    return Err(ttrpc_error(
                        Code::INTERNAL,
                        String::from("failed to get id in exec"),
                    ))
                }
            },
        )?;
        let id = req.id.clone();
        let spec = req.get_spec().clone();
        match init.exec(
            &self.config.path,
            types::ExecConfig {
                id: id.clone(),
                terminal: req.terminal,
                stdin: req.stdin,
                stdout: req.stdout,
                stderr: req.stderr,
                spec,
            },
        ) {
            Ok(exec) => s.processes.insert(id, Box::new(exec)),
            Err(e) => {
                info!("exec failed");
                return Err(e)?;
            }
        };

        Ok(empty::Empty::new())
    }

    fn resize_pty(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::ResizePtyRequest,
    ) -> ttrpc::Result<empty::Empty> {
        info!("resize_pty: {:?}", req);
        if req.id == "" {
            return Err(ttrpc_error(
                Code::INVALID_ARGUMENT,
                String::from("id not provided."),
            ));
        }
        let s = self.service.inner.lock().unwrap();
        match s.processes.get(&req.id) {
            Some(p) => {
                info!("resize process {}: {}x{}", p.pid(), req.width, req.height);
                p.resize(&WinSize {
                    height: req.height as u16,
                    width: req.width as u16,
                    ..Default::default()
                });
                Ok(empty::Empty::new())
            }
            None => {
                info!("container not created: {}", &req.id);
                Err(ttrpc_error(
                    Code::INTERNAL,
                    String::from("resize pty failed."),
                ))
            }
        }
    }

    fn close_io(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::CloseIORequest,
    ) -> ttrpc::Result<empty::Empty> {
        info!("close io: {:?}", req);
        let mut s = self.service.inner.lock().unwrap();
        match s.processes.get_mut(&req.id) {
            Some(p) => p.close_stdin()?,
            None => {}
        }

        Ok(empty::Empty::new())
    }

    fn shim_info(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        _req: empty::Empty,
    ) -> ttrpc::Result<shim::ShimInfoResponse> {
        Ok(shim::ShimInfoResponse {
            shim_pid: std::process::id(),
            ..Default::default()
        })
    }

    fn update(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::UpdateTaskRequest,
    ) -> ttrpc::Result<empty::Empty> {
        info!("update");
        let s = self.service.inner.lock().unwrap();
        if s.id.is_none() {
            return Err(ttrpc_error(
                Code::FAILED_PRECONDITION,
                String::from("container must be created"),
            ));
        }

        if let Some(data) = req.resources.into_option() {
            if let Ok(resources) = serde_json::from_slice::<LinuxResources>(data.get_value()) {
                info!("update: {:?}", resources);
                let init = self.get_init_process(
                    &s.processes,
                    match s.id.as_ref() {
                        Some(id) => id,
                        None => {
                            return Err(ttrpc_error(
                                Code::INTERNAL,
                                String::from("failed to get id in update"),
                            ))
                        }
                    },
                )?;
                init.update(&resources)?;
                return Ok(empty::Empty::new());
            }
        }

        Err(ttrpc_error(
            Code::INTERNAL,
            String::from("update container failed"),
        ))
    }

    fn wait(
        &self,
        _ctx: &ttrpc::TtrpcContext,
        req: shim::WaitRequest,
    ) -> ttrpc::Result<shim::WaitResponse> {
        let pair;
        {
            let mut s = self.service.inner.lock().unwrap();
            match s.processes.get_mut(&req.id) {
                Some(p) => {
                    pair = p.get_wait_pair();
                }
                None => {
                    return Err(ttrpc_error(Code::NOT_FOUND, String::from("wait failed")));
                }
            }
        }

        let &(ref lock, ref condvar) = &*pair;
        let mut exited = lock.lock().unwrap();
        while !*exited {
            exited = condvar.wait(exited).unwrap();
        }

        let mut s = self.service.inner.lock().unwrap();
        match s.processes.get_mut(&req.id) {
            Some(p) => {
                let mut timestamp = protobuf::well_known_types::Timestamp::new();
                if let Some(exited_at) = p.get_exited_at() {
                    set_proto_timestamp(&mut timestamp, &exited_at);
                }
                let ts = protobuf::SingularPtrField::some(timestamp);
                return Ok(shim::WaitResponse {
                    exit_status: p.get_exit_status() as u32,
                    exited_at: ts,
                    ..Default::default()
                });
            }
            None => {
                return Err(ttrpc_error(Code::NOT_FOUND, String::from("wait failed")));
            }
        }
    }
}
