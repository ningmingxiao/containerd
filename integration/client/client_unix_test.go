//go:build !windows

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

package client

import (
	"runtime"
	"testing"

	. "github.com/containerd/containerd"
	"github.com/containerd/containerd/integration/images"
	"github.com/containerd/containerd/platforms"
)

var (
	testImage             = images.Get(images.BusyBox)
	testMultiLayeredImage = images.Get(images.VolumeCopyUp)
	shortCommand          = withProcessArgs("true")
	// NOTE: The TestContainerPids needs two running processes in one
	// container. But busybox:1.36 sh shell, the `sleep` is a builtin.
	//
	// 	/bin/sh -c "type sleep"
	//      sleep is a shell builtin
	//
	// We should use `/bin/sleep` instead of `sleep`. And busybox sh shell
	// will execve directly instead of clone-execve if there is only one
	// command. There will be only one process in container if we use
	// '/bin/sh -c "/bin/sleep inf"'.
	//
	// So we append `&& exit 0` to force sh shell uses clone-execve.
	longCommand = withProcessArgs("/bin/sh", "-c", "/bin/sleep inf && exit 0")
)

func TestImagePullSchema1WithEmptyLayers(t *testing.T) {
	client, err := newClient(t, address)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()

	ctx, cancel := testContext(t)
	defer cancel()

	schema1TestImageWithEmptyLayers := "127.0.0.1:5000/alpine@sha256:6ed4366fc1b6558fe78cd8d3a524e8cb45948d6e9526910fe48cf475cef0a944"
	if runtime.GOARCH == "arm64" {
		schema1TestImageWithEmptyLayers = "127.0.0.1:5000/alpine@sha256:549694ea68340c26d1d85c00039aa11ad835be279bfd475ff4284b705f92c24e"
	}
	_, err = client.Pull(ctx, schema1TestImageWithEmptyLayers, WithPlatform(platforms.DefaultString()), WithSchema1Conversion, WithPullUnpack)
	if err != nil {
		t.Fatal(err)
	}
}
