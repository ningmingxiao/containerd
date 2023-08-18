use machine::{machine, transitions};

machine!(
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum InitState {
        Created,
        CreatedCheckpoint,
        Running,
        Paused,
        Stopped,
        Deleted,
    }
);

machine!(
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum ExecState {
        ExecCreated,
        ExecRunning,
        ExecStopped,
        ExecDeleted,
    }
);

#[derive(Clone, Debug, PartialEq)]
pub struct Start;

#[derive(Clone, Debug, PartialEq)]
pub struct Stop;

#[derive(Clone, Debug, PartialEq)]
pub struct Delete;

#[derive(Clone, Debug, PartialEq)]
pub struct Pause;

transitions!(InitState,
    [
        (Created, Start) => Running,
        (Created, Stop) => Stopped,
        (Created, Delete) => Deleted,

        (CreatedCheckpoint, Start) => Running,
        (CreatedCheckpoint, Stop) => Stopped,
        (CreatedCheckpoint, Delete) => Deleted,

        (Running, Stop) => Stopped,
        (Running, Pause) => Paused,

        (Paused, Start) => Running,
        (Paused, Stop) => Stopped,

        (Stopped, Delete) => Deleted
    ]
);

transitions!(ExecState,
    [
        (ExecCreated, Start) => ExecRunning,
        (ExecCreated, Stop) => ExecStopped,
        (ExecCreated, Delete) => ExecDeleted,

        (ExecRunning, Stop) => ExecStopped,

        (ExecStopped, Delete) => ExecDeleted
    ]
);

impl Created {
    pub fn on_start(self, _: Start) -> Running {
        Running {}
    }

    pub fn on_stop(self, _: Stop) -> Stopped {
        Stopped {}
    }

    pub fn on_delete(self, _: Delete) -> Deleted {
        Deleted {}
    }
}

impl CreatedCheckpoint {
    pub fn on_start(self, _: Start) -> Running {
        Running {}
    }

    pub fn on_stop(self, _: Stop) -> Stopped {
        Stopped {}
    }

    pub fn on_delete(self, _: Delete) -> Deleted {
        Deleted {}
    }
}

impl Running {
    pub fn on_stop(self, _: Stop) -> Stopped {
        Stopped {}
    }

    pub fn on_pause(self, _: Pause) -> Paused {
        Paused {}
    }
}

impl Paused {
    pub fn on_start(self, _: Start) -> Running {
        Running {}
    }

    pub fn on_stop(self, _: Stop) -> Stopped {
        Stopped {}
    }
}

impl Stopped {
    pub fn on_delete(self, _: Delete) -> Deleted {
        Deleted {}
    }
}

impl ExecCreated {
    pub fn on_start(self, _: Start) -> ExecRunning {
        ExecRunning {}
    }

    pub fn on_stop(self, _: Stop) -> ExecStopped {
        ExecStopped {}
    }

    pub fn on_delete(self, _: Delete) -> ExecDeleted {
        ExecDeleted {}
    }
}

impl ExecRunning {
    pub fn on_stop(self, _: Stop) -> ExecStopped {
        ExecStopped {}
    }
}

impl ExecStopped {
    pub fn on_delete(self, _: Delete) -> ExecDeleted {
        ExecDeleted {}
    }
}
