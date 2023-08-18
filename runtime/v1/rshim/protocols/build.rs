use ttrpc_codegen::Codegen;
use ttrpc_codegen::Customize;

fn main() -> std::io::Result<()> {
    let curdir = std::env::current_dir()?;
    let containerd_path = curdir.join("../../../../").canonicalize()?;
    let gopath = containerd_path.join("../../../").canonicalize()?;
    let protos = vec![
        containerd_path.join("runtime/v1/shim/v1/shim.proto"),
        containerd_path.join("runtime/linux/runctypes/runc.proto"),
        containerd_path
            .join("vendor/github.com/gogo/protobuf/protobuf/google/protobuf/empty.proto"),
        containerd_path.join("api/types/mount.proto"),
        containerd_path.join("api/types/task/task.proto"),
    ];

    // Tell Cargo that if the .proto files changed, to rerun this build script.
    println!("cargo:rerun-if-changed=build.rs");
    // protos
    //     .iter()
    //     .for_each(|p| println!("cargo:rerun-if-changed={:?}", &p));

    Codegen::new()
        .out_dir("src")
        .inputs(vec![containerd_path.join("api/events/task.proto")])
        .include(gopath.as_path())
        .include(containerd_path.join("vendor/github.com/gogo/protobuf/"))
        .include(containerd_path.join("vendor/github.com/gogo/protobuf/protobuf/"))
        .rust_protobuf()
        .run()
        .expect("gen code should not be failed");
    std::fs::rename(curdir.join("src/task.rs"), curdir.join("src/events.rs"))?;

    Codegen::new()
        .out_dir("src")
        .inputs(&protos)
        .include(gopath.as_path())
        .include(containerd_path.join("vendor/github.com/gogo/protobuf/"))
        .include(containerd_path.join("vendor/github.com/gogo/protobuf/protobuf/"))
        .rust_protobuf()
        .customize(Customize {
            async_all: false, // It's the key option.
            ..Default::default()
        })
        .run()
        .expect("gen code should not be failed");

    Ok(())
}
