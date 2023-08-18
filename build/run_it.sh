#!/bin/bash


#docker run --rm --net host --privileged -v /it_test:/tmp -e GOPATH=/go --tmpfs /var/lib/containerd-test -v `pwd`/containerd:/go/src/github.com/containerd/containerd -v  `pwd`:/go/src/github.com/opencontainers  -w /go/src/github.com/containerd/containerd containerd-dev-go1.16.12:1.5.9  sh -c "build/run_it.sh"
#install runc
cd /root/go/src/github.com/opencontainers/runc&&make && make install
if [ $? -ne 0 ];then
        exit 1
fi

# rust shim v2 cri-integration
cd /go/src/github.com/containerd/containerd
echo "CONTAINERD_RUNTIME="io.containerd.runc.v2-rs" REPORT_DIR=`pwd` make cri-integration"
CONTAINERD_RUNTIME="io.containerd.runc.v2-rs" REPORT_DIR=`pwd` make cri-integration
if [ $? -ne 0 ];then
        exit 1
fi
mount | grep -E '/run/containerd|/run/netns/cni-' | awk -F" " '{print $3}' | xargs umount
rm -rf /var/lib/containerd-test/* /run/containerd-test/*

# go shim v2 cri-integration
cd /go/src/github.com/containerd/containerd
echo "CONTAINERD_RUNTIME="io.containerd.runc.v2" REPORT_DIR=`pwd` make cri-integration"
CONTAINERD_RUNTIME="io.containerd.runc.v2" REPORT_DIR=`pwd` make cri-integration
if [ $? -ne 0 ];then
        exit 1
fi
mount | grep -E '/run/containerd|/run/netns/cni-' | awk -F" " '{print $3}' | xargs umount
rm -rf /var/lib/containerd-test/* /run/containerd-test/*

# 测试用例需要单独vendor
# 替换测试用例里vender下的containerd为本工程的containerd
rm -rf /home/vendor/github.com/containerd/containerd/*
cp -r /go/src/github.com/containerd/containerd/* /home/vendor/github.com/containerd/containerd/
\cp -r /home/vendor /go/src/github.com/containerd/containerd/integration/client/
rm -rf /go/src/github.com/containerd/containerd/integration/client/vendor/github.com/containerd/containerd/integration

cd /go/src/github.com/containerd/containerd&&make binaries install integration
if [ $? -ne 0 ];then
        rm -rf /go/src/github.com/containerd/containerd/integration/client/vendor
        exit 1
fi
rm -rf /go/src/github.com/containerd/containerd/integration/client/vendor
