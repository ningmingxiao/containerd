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

package archive

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/containerd/log"

	"golang.org/x/sys/unix"
)

func RunCommand(command string, args ...string) (string, error) {
	cmd := exec.Command(command, args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("命令执行失败: %v, 输出: %s", err, strings.TrimSpace(string(output)))
	}
	return string(output), nil
}
func chtimes(path string, atime, mtime time.Time) error {
	var utimes [2]unix.Timespec
	utimes[0] = unix.NsecToTimespec(atime.UnixNano())
	utimes[1] = unix.NsecToTimespec(mtime.UnixNano())

	if err := unix.UtimesNanoAt(unix.AT_FDCWD, path, utimes[0:], unix.AT_SYMLINK_NOFOLLOW); err != nil {
		if _, errdir := os.Stat(filepath.Dir(path)); errdir != nil {
			log.G(context.TODO()).Infof("nmx001 failed call to UtimesNanoAt for %s dir doesn't exist", filepath.Dir(path))
		}
		mount, err := RunCommand("sh", "-c", "cat /proc/self/mountinfo")
		if err == nil {
			log.G(context.TODO()).Infof("nmx001ff mountinfo %s", mount)
		}
		log.G(context.TODO()).Infof("nmx001 failed call to UtimesNanoAt for %s dir exist", filepath.Dir(path))
		return fmt.Errorf("failed call to UtimesNanoAt for %s: %w", path, err)
	}
	return nil
}
