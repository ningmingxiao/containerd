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

package monitor

import (
	"context"
	"fmt"
	"strconv"
	"sync"
	"time"

	"github.com/containerd/containerd"
	"github.com/containerd/containerd/namespaces"
	"github.com/containerd/containerd/plugin"
	"github.com/containerd/containerd/runtime/restart"
	"github.com/sirupsen/logrus"
)

type duration struct {
	time.Duration
}

func (d *duration) UnmarshalText(text []byte) error {
	var err error
	d.Duration, err = time.ParseDuration(string(text))
	return err
}

func (d duration) MarshalText() ([]byte, error) {
	return []byte(d.Duration.String()), nil
}

// Config for the restart monitor
type Config struct {
	// Interval for how long to wait to check for state changes
	Interval duration `toml:"interval"`
}

func init() {
	plugin.Register(&plugin.Registration{
		Type: plugin.InternalPlugin,
		Requires: []plugin.Type{
			plugin.EventPlugin,
			plugin.ServicePlugin,
		},
		ID: "restart",
		Config: &Config{
			Interval: duration{
				Duration: 10 * time.Second,
			},
		},
		InitFn: func(ic *plugin.InitContext) (interface{}, error) {
			ic.Meta.Capabilities = []string{"no", "always", "on-failure", "unless-stopped"}
			client, err := containerd.New("", containerd.WithInMemoryServices(ic))
			if err != nil {
				return nil, err
			}
			m := &monitor{
				client: client,
			}
			go m.run(ic.Config.(*Config).Interval.Duration)
			return m, nil
		},
	})
}

type change interface {
	apply(context.Context, *containerd.Client) error
}

type monitor struct {
	client         *containerd.Client
	reconcileTimes uint64
}

func (m *monitor) run(interval time.Duration) {
	if interval == 0 {
		interval = 10 * time.Second
	}
	for {
		if err := m.reconcile(context.Background()); err != nil {
			logrus.WithError(err).Error("reconcile")
		}
		time.Sleep(interval)
	}
}

func (m *monitor) reconcile(ctx context.Context) error {

	//if m.reconcileTimes value is 1 means reconcile run at first time
	//else if m.reconcileTimes >= 2 means reconcile run more than twice
	if m.reconcileTimes < 2 {
		m.reconcileTimes++
	}
	ns, err := m.client.NamespaceService().List(ctx)
	if err != nil {
		return err
	}
	var wgNSLoop sync.WaitGroup
	for _, name := range ns {
		name := name
		wgNSLoop.Add(1)
		go func() {
			defer wgNSLoop.Done()
			ctx := namespaces.WithNamespace(ctx, name)
			changes, err := m.monitor(ctx)
			if err != nil {
				logrus.WithError(err).Error("monitor for changes")
				return
			}
			var wgChangesLoop sync.WaitGroup
			for _, c := range changes {
				c := c
				wgChangesLoop.Add(1)
				go func() {
					defer wgChangesLoop.Done()
					if err := c.apply(ctx, m.client); err != nil {
						logrus.WithError(err).Error("apply change")
					}
				}()
			}
			wgChangesLoop.Wait()
		}()
	}
	wgNSLoop.Wait()
	return nil
}

func (m *monitor) monitor(ctx context.Context) ([]change, error) {
	containers, err := m.client.Containers(ctx, fmt.Sprintf("labels.%q", restart.StatusLabel))
	if err != nil {
		return nil, err
	}
	var changes []change
	for _, c := range containers {
		var (
			task   containerd.Task
			status containerd.Status
			err    error
		)
		labels, err := c.Labels(ctx)
		if err != nil {
			return nil, err
		}
		//restart unrunning container if container's  restart strategy is restart=always
		if m.reconcileTimes == 1 {
			rp, err := restart.NewPolicy(labels[restart.PolicyLabel])
			if err != nil {
				return nil, err
			}
			task, err := c.Task(ctx, nil)
			if err != nil {
				return nil, err
			}
			status, err := task.Status(ctx)
			if err != nil {
				return nil, err
			}
			if rp.Name() == "always" &&
				status.Status == containerd.Stopped ||
				status.Status == containerd.Created ||
				status.Status == containerd.Unknown {
				if err := UpdateContainerStoppedLabel(ctx, c, false); err != nil {
					return nil, err
				}
			}
		}

		desiredStatus := containerd.ProcessStatus(labels[restart.StatusLabel])
		if task, err = c.Task(ctx, nil); err == nil {
			if status, err = task.Status(ctx); err == nil {
				if desiredStatus == status.Status {
					continue
				}
			}
		}

		// Task or Status return error, only desired to running
		if err != nil {
			logrus.WithError(err).Error("monitor")
			if desiredStatus == containerd.Stopped {
				continue
			}
		}

		// Known issue:
		// The status may be empty when task failed but was deleted,
		// which will result in an `on-failure` restart policy reconcile error.
		switch desiredStatus {
		case containerd.Running:
			if !restart.Reconcile(status, labels) {
				continue
			}
			restartCount, _ := strconv.Atoi(labels[restart.CountLabel])
			if labels["containerd.io/restart.logpath"] != "" {
				logrus.Warn(`Label "containerd.io/restart.logpath" is no longer supported since containerd v2.0. Use "containerd.io/restart.loguri" instead.`)
			}
			changes = append(changes, &startChange{
				container: c,
				logURI:    labels[restart.LogURILabel],
				count:     restartCount + 1,
			})
		case containerd.Stopped:
			changes = append(changes, &stopChange{
				container: c,
			})
		}
	}
	return changes, nil
}

func UpdateContainerStoppedLabel(ctx context.Context, container containerd.Container, stopped bool) error {
	opt := containerd.WithAdditionalContainerLabels(map[string]string{
		restart.ExplicitlyStoppedLabel: strconv.FormatBool(stopped),
	})
	return container.Update(ctx, containerd.UpdateContainerOpts(opt))
}
