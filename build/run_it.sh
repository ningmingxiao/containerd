#!/bin/bash

cd /go/src/github.com/containerd/containerd
sh ./script/setup/install-failpoint-binaries
##docker run --rm --net host --privileged -v /it_test:/tmp -e GOPATH=/go --tmpfs /var/lib/containerd-test -v `pwd`/containerd:/go/src/github.com/containerd/containerd -v  `pwd`:/go/src/github.com/opencontainers  -w /go/src/github.com/containerd/containerd containerd-dev-go1.16.12:1.5.9  sh -c "build/run_it.sh"
##install runc
cd /root/go/src/github.com/opencontainers/runc&&make && make install
if [ $? -ne 0 ];then
        exit 1
fi

rm -rf /home/vendor/github.com/containerd/containerd
mv /go/src/github.com/containerd/containerd/integration/client/go.mod  /home/
mv /go/src/github.com/containerd/containerd/integration/client/go.sum  /home/
cp -r /home/vendor /go/src/github.com/containerd/containerd/integration/client/

function clean() {
  rm -rf /go/src/github.com/containerd/containerd/integration/client/vendor
  cd  /home/ && cp go.sum  go.mod /go/src/github.com/containerd/containerd/integration/client/
}

cd /go/src/github.com/containerd/containerd && make binaries install
#if [ $? -ne 0 ];then
#        exit 1
#fi

# go shim v2 cri-integration
echo "***********test-for-goshimv2***********"
cd /go/src/github.com/containerd/containerd &&\
CONTAINERD_RUNTIME="io.containerd.runc.v2" REPORT_DIR=`pwd` make cri-integration
#if [ $? -ne 0 ];then
#        exit 1
#fi
#mount | grep -E '/run/containerd|/run/netns/cni-' | awk -F" " '{print $3}' | xargs umount
#rm -rf /var/lib/containerd-test/* /run/containerd-test/*

echo "***********test-for-rustshimv2***********"
cd /go/src/github.com/containerd/containerd &&\
CONTAINERD_RUNTIME="io.containerd.runc.v2-rs" REPORT_DIR=`pwd` make  cri-integration
#if [ $? -ne 0 ];then
#        exit 1
#fi

cd /go/src/github.com/containerd/containerd && make integration
#if [ $? -ne 0 ];then
#        clean
#        exit 1
#fi

rm -rf /var/lib/containerd-test/* /run/containerd-test/*
clean
