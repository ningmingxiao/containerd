// +build !libcontainer,!runc

package containerd

import (
	"errors"

	"github.com/docker/containerd/runtime"
)

func newRuntime(stateDir string) (runtime.Runtime, error) {
	return nil, errors.New("unsupported platform")
}
