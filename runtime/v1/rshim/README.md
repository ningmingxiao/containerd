# shim v1

rust版本的containerd shim v1， 二进制名称为containerd-rshim. 可以通过containerd配置文件配置使用该shim:

```toml
[plugins]
  [plugins.linux]
    shim = "containerd-rshim"
    runtime = "runc"
    runtime_root = "/var/lib/docker/runc"
    no_shim = false
    shim_debug = true
```

如果需要查看日志， 需要将shim_debug配置项设置为true， 这样shim日志会打印到pipe文件中， containerd中会从pipe文件中读取日志。
