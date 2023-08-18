use std::collections::HashMap;

pub struct Arguments {
    params: HashMap<String, Value>,
}

pub enum Value {
    Opt(Option<String>),
    Flag(bool),
}

impl Arguments {
    pub fn new() -> Self {
        let mut p = Arguments {
            params: HashMap::new(),
        };

        p.params.insert(String::from("namespace"), Value::Opt(None));
        p.params.insert(String::from("workdir"), Value::Opt(None));
        p.params.insert(String::from("address"), Value::Opt(None));
        p.params
            .insert(String::from("containerd-binary"), Value::Opt(None));
        p.params.insert(String::from("criu-path"), Value::Opt(None));
        p.params.insert(
            String::from("runtime-root"),
            Value::Opt(Some(String::from("/run/containerd/runc"))),
        );
        p.params
            .insert(String::from("systemd-cgroup"), Value::Flag(false));
        p.params.insert(String::from("debug"), Value::Flag(false));

        p
    }

    pub fn parse<A: Iterator<Item = String>>(&mut self, args: A) {
        let mut args = args.skip(1);
        while let Some(arg) = args.next() {
            if arg.starts_with("-") {
                let arg = &arg[1..];

                match self.params.get_mut(arg) {
                    Some(&mut Value::Opt(ref mut v)) => {
                        *v = args.next();
                    }
                    Some(&mut Value::Flag(ref mut v)) => {
                        *v = true;
                    }
                    _ => {
                        self.help(arg);
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    pub fn value_of(&self, name: &str) -> Option<&str> {
        match self.params.get(name) {
            Some(&Value::Opt(Some(ref v))) => Some(v),
            _ => None,
        }
    }

    pub fn is_present(&self, name: &str) -> bool {
        match self.params.get(name) {
            Some(&Value::Flag(v)) => v,
            _ => false,
        }
    }

    fn help(&self, unknown_flag: &str) {
        let msg = format!(
            "flag provided but not defined: -{}
Usage of containerd-shim:
  -address string
        grpc address back to main containerd
  -containerd-binary containerd publish
        path to containerd binary (used for containerd publish) (default \"containerd\")
  -criu string
        path to criu binary
  -debug
        enable debug output in logs
  -namespace string
        namespace that owns the shim
  -runtime-root string
        root directory for the runtime (default \"/run/containerd/runc\")
  -socket string
        abstract socket path to serve
  -systemd-cgroup
        set runtime to use systemd-cgroup
  -workdir string
        path used to storge large temporary data
",
            unknown_flag
        );
        print!("{}", msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_1() {
        let mut argparser = Arguments::new();
        let args = vec!["containerd-shim", "-namespace", "moby", "-workdir", "/var/lib/containerd/io.containerd.runtime.v1.linux/moby/2bb5980095a8be31ad5436054e01c112dcefef25adef4b3cfac70053ff901f0a", "-address", "/run/containerd/containerd.sock", "-containerd-binary", "/usr/bin/containerd", "-runtime-root", "/var/run/docker/runtime-runc", "-debug"];
        let a = args.into_iter().map(|v| v.to_string());
        argparser.parse(a);

        if let Some(Value::Opt(Some(v))) = argparser.params.get("namespace") {
            assert_eq!(*v, "moby");
        }

        if let Some(Value::Opt(Some(v))) = argparser.params.get("workdir") {
            assert_eq!(*v, "/var/lib/containerd/io.containerd.runtime.v1.linux/moby/2bb5980095a8be31ad5436054e01c112dcefef25adef4b3cfac70053ff901f0a");
        }

        if let Some(Value::Opt(Some(v))) = argparser.params.get("address") {
            assert_eq!(*v, "/run/containerd/containerd.sock");
        }

        if let Some(Value::Opt(Some(v))) = argparser.params.get("containerd-binary") {
            assert_eq!(*v, "/usr/bin/containerd");
        }

        if let Some(Value::Opt(Some(v))) = argparser.params.get("runtime-root") {
            assert_eq!(*v, "/var/run/docker/runtime-runc");
        }

        if let Some(Value::Flag(v)) = argparser.params.get("debug") {
            assert_eq!(*v, true);
        }

        if let Some(Value::Flag(v)) = argparser.params.get("systemd-cgroup") {
            assert_eq!(*v, false);
        }
    }

    #[test]
    fn test_2() {
        let mut argparser = Arguments::new();
        let args = vec!["containerd-shim", "-namespace", "moby", "-workdir", "/var/lib/containerd/io.containerd.runtime.v1.linux/moby/2bb5980095a8be31ad5436054e01c112dcefef25adef4b3cfac70053ff901f0a", "-address", "/run/containerd/containerd.sock", "-containerd-binary", "/usr/bin/containerd", "-runtime-root", "/var/run/docker/runtime-runc", "-debug"];
        let a = args.into_iter().map(|v| v.to_string());
        argparser.parse(a);

        let ns = argparser.value_of("namespace");
        assert_eq!(Some("moby"), ns);

        let workdir = argparser.value_of("workdir");
        assert_eq!(Some("/var/lib/containerd/io.containerd.runtime.v1.linux/moby/2bb5980095a8be31ad5436054e01c112dcefef25adef4b3cfac70053ff901f0a"), workdir);

        let address = argparser.value_of("address");
        assert_eq!(Some("/run/containerd/containerd.sock"), address);

        let bin = argparser.value_of("containerd-binary");
        assert_eq!(Some("/usr/bin/containerd"), bin);

        let root = argparser.value_of("runtime-root");
        assert_eq!(Some("/var/run/docker/runtime-runc"), root);

        let debug = argparser.is_present("debug");
        assert_eq!(true, debug);

        let sc = argparser.is_present("systemd-cgroup");
        assert_eq!(false, sc);
    }
}
