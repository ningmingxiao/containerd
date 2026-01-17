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

package integration

import (
	"os"
	"runtime"
	"sync"
	"testing"
	"time"

	"github.com/containerd/containerd/v2/integration/images"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	criRuntime "k8s.io/cri-api/pkg/apis/runtime/v1"
)

func TestContainerUpdate(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("it seems that windows platform doesn't support detached process. skip it")
	}
	if f := os.Getenv("RUNC_FLAVOR"); f != "" && f != "runc" {
		t.Skip("test requires runc")
	}
	containerMap := make(map[string]string)
	for _, sandbox := range []string{"sandbox-1", "sandbox-2"} {
		t.Log("Create a sandbox")
		sb, sbConfig := PodSandboxConfigWithCleanup(t, sandbox, "container-status-update")
		var (
			testImage     = images.Get(images.BusyBox)
			containerName = "test-container-update"
		)

		EnsureImageExists(t, testImage)

		cnConfig := &criRuntime.ContainerConfig{}
		if sandbox == "sandbox-1" {
			annonations := map[string]string{
				"oci.runc.failpoint.profile": "Update",
			}
			cnConfig = ContainerConfig(
				containerName,
				testImage,
				WithAnnotations(annonations),
				WithCommand("sh", "-c", "sleep 365d"),
			)
		} else {
			cnConfig = ContainerConfig(
				containerName,
				testImage,
				WithCommand("sh", "-c", "sleep 365d"),
			)
		}
		cn, err := runtimeService.CreateContainer(sb, cnConfig, sbConfig)
		require.NoError(t, err)
		defer func() {
			assert.NoError(t, runtimeService.RemoveContainer(cn))
		}()
		t.Log("Start the container")
		require.NoError(t, runtimeService.StartContainer(cn))
		containerMap[sandbox] = cn
		defer func() {
			assert.NoError(t, runtimeService.StopContainer(cn, 10))
		}()
	}
	var errUpdateSandbox error
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		errUpdateSandbox = runtimeService.UpdateContainerResources(containerMap["sandbox-1"], &criRuntime.LinuxContainerResources{
			MemoryLimitInBytes: int64(256 * 1024 * 1024),
		}, nil)
		wg.Done()
	}()
	time.Sleep(time.Second * 1)
	assert.NoError(t, errUpdateSandbox)
	t1 := time.Now()
	err := runtimeService.UpdateContainerResources(containerMap["sandbox-2"], &criRuntime.LinuxContainerResources{
		MemoryLimitInBytes: int64(256 * 1024 * 1024),
	}, nil)
	assert.NoError(t, err)
	duration := time.Since(t1)
	wg.Wait()
	if duration > 2*time.Second {
		t.Fatalf("update container use %v", duration)
	}
	t.Logf("update container use %v", duration)
}
