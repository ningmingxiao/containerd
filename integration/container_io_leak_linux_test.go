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
	"bytes"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/containerd/containerd/v2/integration/images"
	"github.com/stretchr/testify/require"
)

func TestContainerIoLeakAfterExit(t *testing.T) {
	t.Skip("test requires runc")
	if f := os.Getenv("RUNC_FLAVOR"); f != "" && f != "runc" {
		t.Skip("test requires runc")
	}
	t.Log("Create a sandbox")
	sb, sbConfig := PodSandboxConfigWithCleanup(t, "sandbox", "container-io-leak-after-exit")
	testImage := images.Get(images.BusyBox)
	EnsureImageExists(t, testImage)
	var testcases = []struct {
		name  string
		stdin bool
	}{
		{
			name:  "ttyOnly",
			stdin: false,
		},
	}
	for _, testcase := range testcases {
		t.Run(testcase.name, func(t *testing.T) {
			t.Log("Create a container")
			cnConfig := ContainerConfig(
				testcase.name,
				testImage,
				WithCommand("sh", "-c", "sleep 365d"),
			)
			cnConfig.Stdin = testcase.stdin
			cnConfig.Tty = true
			t.Log("Create the container")
			cn, err := runtimeService.CreateContainer(sb, cnConfig, sbConfig)
			require.NoError(t, err)
			// generate oci runtime failed error
			runcPath, err := getRuncPath()
			require.NoError(t, err)
			dir := filepath.Dir(runcPath)
			err = copyFile(runcPath, filepath.Join(os.TempDir(), "runc-fp.v1"))
			require.NoError(t, err)
			err = os.RemoveAll(runcPath)
			require.NoError(t, err)
			defer func() {
				copyFile(filepath.Join(os.TempDir(), "runc-fp.v1"), runcPath)
			}()
			lsResoult, _ := exec.Command("sh", "-c", fmt.Sprintf("ls %s", dir)).CombinedOutput()
			t.Logf("ls resoult is: %s", lsResoult)
			t.Log("Start the container")
			require.Error(t, runtimeService.StartContainer(cn))
			pid := getShimPid(t, sb)
			t.Logf("numPipe is %d", numPipe(pid))
		})
	}
}
func copyFile(src, dst string) error {
	srcFile, err := os.Open(src)
	if err != nil {
		return fmt.Errorf("open source file: %w", err)
	}
	defer srcFile.Close()
	dstFile, err := os.Create(dst)
	if err != nil {
		return fmt.Errorf("create dest file: %w", err)
	}
	defer dstFile.Close()
	_, err = io.Copy(dstFile, srcFile)
	return err
}
func getRuncPath() (string, error) {
	runcPath, err := exec.LookPath("runc")
	if err != nil {
		return "", err
	}
	return runcPath, nil
}
func numPipe(shimPid int) int {
	cmd := exec.Command("sh", "-c", fmt.Sprintf("lsof -p %d | grep pipe", shimPid))
	var stdout bytes.Buffer
	cmd.Stdout = &stdout
	if err := cmd.Run(); err != nil {
		return 0
	}
	return strings.Count(stdout.String(), "\n")
}
