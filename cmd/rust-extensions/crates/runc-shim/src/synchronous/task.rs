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

use std::collections::HashMap;
use std::process;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, Once};

use log::{debug, info, warn};
use oci_spec::runtime::LinuxResources;

use containerd_shim as shim;

use shim::api::*;
use shim::event::Event;
use shim::protos::events::task::{
    TaskCreate, TaskDelete, TaskExecAdded, TaskExecStarted, TaskIO, TaskStart,TaskOOM,
};
use shim::protos::protobuf::{Message, SingularPtrField};
use shim::util::{convert_to_any, convert_to_timestamp, IntoOption};
use shim::{other_error, Error, ExitSignal, Task, TtrpcContext, TtrpcResult};
use cgroups_rs::memory::MemController;
use std::sync::mpsc::Receiver;
use containerd_shim::protos::ttrpc;

use crate::synchronous::container::{Container, ContainerFactory};

type EventSender = Sender<(String, Box<dyn Message>)>;

pub struct ShimTask<F, C> {
    pub containers: Arc<Mutex<HashMap<String, C>>>,
    factory: F,
    namespace: String,
    exit: Arc<ExitSignal>,
    /// Prevent multiple shutdown
    shutdown: Once,
    tx: Arc<Mutex<EventSender>>,
}

impl<F, C> ShimTask<F, C>
where
    F: Default,
{
    pub fn new(ns: &str, exit: Arc<ExitSignal>, tx: EventSender) -> Self {
        Self {
            factory: Default::default(),
            containers: Arc::new(Mutex::new(Default::default())),
            namespace: ns.to_string(),
            exit,
            shutdown: Once::new(),
            tx: Arc::new(Mutex::new(tx)),
        }
    }
}

impl<F, C> ShimTask<F, C> {
    pub fn send_event(&self, event: impl Event) {
        let topic = event.topic();
        self.tx
            .lock()
            .unwrap()
            .send((topic.to_string(), Box::new(event)))
            .unwrap_or_else(|e| warn!("send {} to publisher: {}", topic, e));
    }
}

pub fn run_oom_monitor(rx: Receiver<String>, id: String, tx: Arc<Mutex<EventSender>>) {
    let oom_event = TaskOOM{
        container_id: id,
        ..Default::default()
    };

    let topic = oom_event.topic();
    let oom_box = Box::new(oom_event);

    std::thread::spawn(move || {
        for _s in rx.iter() {
            tx.lock()
            .unwrap()
            .send((topic.to_string(), oom_box.clone()))
            .unwrap_or_else(|e| warn!("send {} to publisher: {}", topic, e));
        }
    });
}

impl<F, C> Task for ShimTask<F, C>
where
    F: ContainerFactory<C>,
    C: Container,
{
    fn state(&self, _ctx: &TtrpcContext, req: StateRequest) -> TtrpcResult<StateResponse> {
        let containers = self.containers.lock().unwrap();
        let container = containers.get(req.id.as_str()).ok_or_else(|| {
            Error::NotFoundError(format!("can not find container by id {}", req.id.as_str()))
        })?;
        let exec_id = req.exec_id.as_str().none_if(|&x| x.is_empty());
        let resp = container.state(exec_id)?;
        Ok(resp)
    }

    fn create(
        &self,
        _ctx: &TtrpcContext,
        req: CreateTaskRequest,
    ) -> TtrpcResult<CreateTaskResponse> {
        info!("Create request for {:?}", &req);
        // Note: Get containers here is for getting the lock,
        // to make sure no other threads manipulate the containers metadata;
        let mut containers = self.containers.lock().unwrap();

        let ns = self.namespace.as_str();
        let id = req.id.as_str();

        let container = self.factory.create(ns, &req)?;
        let mut resp = CreateTaskResponse::new();
        let pid = container.pid() as u32;
        resp.pid = pid;

        containers.insert(id.to_string(), container);

        self.send_event(TaskCreate {
            container_id: req.id.to_string(),
            bundle: req.bundle.to_string(),
            rootfs: req.rootfs,
            io: SingularPtrField::some(TaskIO {
                stdin: req.stdin.to_string(),
                stdout: req.stdout.to_string(),
                stderr: req.stderr.to_string(),
                terminal: req.terminal,
                unknown_fields: Default::default(),
                cached_size: Default::default(),
            }),
            checkpoint: req.checkpoint.to_string(),
            pid,
            ..Default::default()
        });

        info!("Create request for {} returns pid {}", id, pid);
        Ok(resp)
    }

    fn start(&self, _ctx: &TtrpcContext, req: StartRequest) -> TtrpcResult<StartResponse> {
        info!("Start request for {:?}", &req);
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::NotFoundError(format!("can not find container by id {}", req.get_id()))
        })?;
        let pid = container.start(req.exec_id.as_str().none_if(|&x| x.is_empty()))?;

        let mut resp = StartResponse::new();
        resp.pid = pid as u32;

        if req.exec_id.is_empty() {
            let cg = container.cgroup();
            let mem: &MemController = cg.controller_of()
                            .expect("No memory controller attached!");
            let rx = match mem.register_oom_event(&req.id) {
                Ok(rx) => rx,
                Err(e) => return Err(ttrpc::Error::Others(format!("register_oom_event failed: {}",  e))),
            };

            run_oom_monitor(rx, req.id.to_string(), self.tx.clone());
            self.send_event(TaskStart {
                container_id: req.id.to_string(),
                pid: pid as u32,
                ..Default::default()
            });
        } else {
            self.send_event(TaskExecStarted {
                container_id: req.id.to_string(),
                exec_id: req.exec_id.to_string(),
                pid: pid as u32,
                ..Default::default()
            });
        };

        info!("Start request for {:?} returns pid {}", req, resp.get_pid());
        Ok(resp)
    }

    fn delete(&self, _ctx: &TtrpcContext, req: DeleteRequest) -> TtrpcResult<DeleteResponse> {
        info!("Delete request for {:?}", &req);
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::NotFoundError(format!("can not find container by id {}", req.get_id()))
        })?;
        let id = container.id();
        let exec_id_opt = req.get_exec_id().none_if(|x| x.is_empty());
        let (pid, exit_status, exited_at) = container.delete(exec_id_opt)?;
        if req.get_exec_id().is_empty() {
            containers.remove(req.id.as_str());
        }

        let ts = convert_to_timestamp(exited_at);
        self.send_event(TaskDelete {
            container_id: id,
            pid: pid as u32,
            exit_status: exit_status as u32,
            exited_at: SingularPtrField::some(ts.clone()),
            ..Default::default()
        });

        let mut resp = DeleteResponse::new();
        resp.set_exited_at(ts);
        resp.set_pid(pid as u32);
        resp.set_exit_status(exit_status as u32);
        info!(
            "Delete request for {} {} returns {:?}",
            req.get_id(),
            req.get_exec_id(),
            resp
        );
        Ok(resp)
    }

    fn pids(&self, _ctx: &TtrpcContext, req: PidsRequest) -> TtrpcResult<PidsResponse> {
        debug!("Pids request for {:?}", req);
        let containers = self.containers.lock().unwrap();
        let container = containers.get(req.get_id()).ok_or_else(|| {
            Error::Other(format!("can not find container by id {}", req.get_id()))
        })?;

        let resp = container.pids()?;
        Ok(resp)
    }

    fn kill(&self, _ctx: &TtrpcContext, req: KillRequest) -> TtrpcResult<Empty> {
        info!("Kill request for {:?}", req);
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::NotFoundError(format!("can not find container by id {}", req.get_id()))
        })?;
        container.kill(
            req.exec_id.as_str().none_if(|&x| x.is_empty()),
            req.signal,
            req.all,
        )?;
        info!("Kill request for {:?} returns successfully", req);
        Ok(Empty::new())
    }

    fn exec(&self, _ctx: &TtrpcContext, req: ExecProcessRequest) -> TtrpcResult<Empty> {
        let exec_id = req.get_exec_id().to_string();
        info!(
            "Exec request for id: {} exec_id: {}",
            req.get_id(),
            req.get_exec_id()
        );
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::Other(format!("can not find container by id {}", req.get_id()))
        })?;
        container.exec(req)?;

        self.send_event(TaskExecAdded {
            container_id: container.id(),
            exec_id,
            ..Default::default()
        });

        Ok(Empty::new())
    }

    fn resize_pty(&self, _ctx: &TtrpcContext, req: ResizePtyRequest) -> TtrpcResult<Empty> {
        debug!(
            "Resize pty request for container {}, exec_id: {}",
            &req.id, &req.exec_id
        );
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::Other(format!("can not find container by id {}", req.get_id()))
        })?;
        container.resize_pty(
            req.get_exec_id().none_if(|&x| x.is_empty()),
            req.height,
            req.width,
        )?;
        Ok(Empty::new())
    }

    fn close_io(&self, _ctx: &TtrpcContext, _req: CloseIORequest) -> TtrpcResult<Empty> {
        // unnecessary close io here since fd was closed automatically after object was destroyed.
        Ok(Empty::new())
    }

    fn update(&self, _ctx: &TtrpcContext, req: UpdateTaskRequest) -> TtrpcResult<Empty> {
        debug!("Update request for {:?}", req);
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::Other(format!("can not find container by id {}", req.get_id()))
        })?;

        let resources: LinuxResources = serde_json::from_slice(req.get_resources().get_value())
            .map_err(other_error!(e, "failed to parse spec"))?;
        container.update(&resources)?;
        Ok(Empty::new())
    }

    fn wait(&self, _ctx: &TtrpcContext, req: WaitRequest) -> TtrpcResult<WaitResponse> {
        info!("Wait request for {:?}", req);
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::Other(format!("can not find container by id {}", req.get_id()))
        })?;
        let exec_id = req.exec_id.as_str().none_if(|&x| x.is_empty());
        let state = container.state(exec_id)?;
        if state.status != Status::RUNNING && state.status != Status::CREATED {
            let mut resp = WaitResponse::new();
            resp.exit_status = state.exit_status;
            resp.exited_at = state.exited_at;
            info!("Wait request for {:?} returns {:?}", req, &resp);
            return Ok(resp);
        }
        let rx = container.wait_channel(req.exec_id.as_str().none_if(|&x| x.is_empty()))?;
        // release the lock before waiting the channel
        drop(containers);

        rx.recv()
            .expect_err("wait channel should be closed directly");
        // get lock again.
        let mut containers = self.containers.lock().unwrap();
        let container = containers.get_mut(req.get_id()).ok_or_else(|| {
            Error::Other(format!("can not find container by id {}", req.get_id()))
        })?;
        let (_, code, exited_at) = container.get_exit_info(exec_id)?;
        let mut resp = WaitResponse::new();
        resp.exit_status = code as u32;
        let ts = convert_to_timestamp(exited_at);
        resp.exited_at = SingularPtrField::some(ts);
        info!("Wait request for {:?} returns {:?}", req, &resp);
        Ok(resp)
    }

    fn stats(&self, _ctx: &TtrpcContext, req: StatsRequest) -> TtrpcResult<StatsResponse> {
        debug!("Stats request for {:?}", req);
        let containers = self.containers.lock().unwrap();
        let container = containers.get(req.get_id()).ok_or_else(|| {
            Error::Other(format!("can not find container by id {}", req.get_id()))
        })?;
        let stats = container.stats()?;

        let mut resp = StatsResponse::new();
        resp.set_stats(convert_to_any(Box::new(stats))?);
        Ok(resp)
    }

    fn shutdown(&self, _ctx: &TtrpcContext, _req: ShutdownRequest) -> TtrpcResult<Empty> {
        debug!("Shutdown request");
        let containers = self.containers.lock().unwrap();
        if containers.len() > 0 {
            return Ok(Empty::new());
        }

        self.shutdown.call_once(|| {
            self.exit.signal();
        });

        Ok(Empty::default())
    }

    fn connect(&self, _ctx: &TtrpcContext, req: ConnectRequest) -> TtrpcResult<ConnectResponse> {
        info!("Connect request for {:?}", req);

        let containers = self.containers.lock().unwrap();
        let container = containers.get(req.get_id()).ok_or_else(|| {
            Error::NotFoundError(format!("can not find container by id {}", req.get_id()))
        })?;

        let resp = ConnectResponse {
            shim_pid: process::id() as u32,
            task_pid: container.pid() as u32,
            ..Default::default()
        };

        Ok(resp)
    }
}

