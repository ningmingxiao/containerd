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
	"errors"
	"fmt"
	"os"
	"testing"
	"time"

	"github.com/containerd/containerd/v2/integration/images"
	"github.com/stretchr/testify/require"
)

func TestRuncLeakWithShimKilled(t *testing.T) {
	t.Log("Create a container")
	testImage := images.Get(images.BusyBox)
	cnConfig := ContainerConfig(
		"test-container-dir-leak",
		testImage,
		WithCommand("sh", "-c", "sleep 365d"),
	)
	t.Log("Create the container")
	sb, sbConfig := PodSandboxConfigWithCleanup(t, "sandbox", "test-container-dir-leak-after-shimkilled")
	cn, err := runtimeService.CreateContainer(sb, cnConfig, sbConfig)
	require.NoError(t, err)
	t.Log("Start the container")
	require.NoError(t, runtimeService.StartContainer(cn))
	dir := fmt.Sprintf("/run/containerd/runc/k8s.io/%s", cn)
	_, err = os.Stat(dir)
	if err != nil {
		t.Fatalf("dir %s should not be empty", dir)
	}
	pid := getShimPid(t, sb)
	KillPid(pid)
	time.Sleep(time.Second * 3)
	_, err = os.Stat(dir)
	if err == nil {
		t.Fatal("err can't be nil")
	} else {
		if !errors.Is(err, os.ErrNotExist) {
			t.Fatalf("dir %s should be empty", dir)
		}
	}
}
