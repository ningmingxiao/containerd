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
	"fmt"
	"testing"

	. "github.com/containerd/containerd/v2/client"
	"github.com/containerd/containerd/v2/pkg/oci"
)

func BenchmarkContainerCreate(b *testing.B) {
	client, err := newClient(b, address)
	if err != nil {
		b.Fatal(err)
	}
	defer client.Close()

	ctx, cancel := testContext(b)
	defer cancel()

	image, err := client.GetImage(ctx, testImage)
	if err != nil {
		b.Error(err)
		return
	}
	var containers []Container
	defer func() {
		for _, c := range containers {
			if err := c.Delete(ctx, WithSnapshotCleanup); err != nil {
				b.Error(err)
			}
		}
	}()

	// reset the timer before creating containers
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		id := fmt.Sprintf("%s-%d", b.Name(), i)
		container, err := client.NewContainer(ctx, id, WithNewSnapshot(id, image), WithNewSpec(oci.WithImageConfig(image), withExitStatus(7)))
		if err != nil {
			b.Error(err)
			return
		}
		containers = append(containers, container)
	}
	b.StopTimer()
}

func BenchmarkContainerStart(b *testing.B) {
	client, err := newClient(b, address)
	if err != nil {
		b.Fatal(err)
	}
	defer client.Close()

	ctx, cancel := testContext(b)
	defer cancel()

	image, err := client.GetImage(ctx, testImage)
	if err != nil {
		b.Error(err)
		return
	}
	var containers []Container
	defer func() {
		for _, c := range containers {
			if err := c.Delete(ctx, WithSnapshotCleanup); err != nil {
				b.Error(err)
			}
		}
	}()

	for i := 0; i < b.N; i++ {
		id := fmt.Sprintf("%s-%d", b.Name(), i)
		container, err := client.NewContainer(ctx, id, WithNewSnapshot(id, image), WithNewSpec(oci.WithImageConfig(image), withExitStatus(7)))
		if err != nil {
			b.Error(err)
			return
		}
		containers = append(containers, container)

	}
	// reset the timer before starting tasks
	b.ResetTimer()
	for _, c := range containers {
		task, err := c.NewTask(ctx, empty())
		if err != nil {
			b.Error(err)
			return
		}
		statusC, err := task.Wait(ctx)
		if err != nil {
			b.Error(err)
			return
		}
		defer task.Delete(ctx)
		if err := task.Start(ctx); err != nil {
			b.Error(err)
			return
		}
		status := <-statusC
		code, _, err := status.Result()
		if err != nil {
			b.Error(err)
			return
		}
		if code != 7 {
			b.Errorf("expected status 7 from wait but received %d", code)
		}
	}
	b.StopTimer()
}
