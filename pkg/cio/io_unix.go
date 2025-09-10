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

package cio

import (
	"context"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"sync"
	"syscall"

	"github.com/containerd/fifo"
	"github.com/containerd/log"
)

// NewFIFOSetInDir returns a new FIFOSet with paths in a temporary directory under root
func NewFIFOSetInDir(root, id string, terminal bool) (*FIFOSet, error) {
	if root != "" {
		if err := os.MkdirAll(root, 0700); err != nil {
			return nil, err
		}
	}
	dir, err := os.MkdirTemp(root, "")
	if err != nil {
		return nil, err
	}
	closer := func() error {
		return os.RemoveAll(dir)
	}
	return NewFIFOSet(Config{
		Stdin:    filepath.Join(dir, id+"-stdin"),
		Stdout:   filepath.Join(dir, id+"-stdout"),
		Stderr:   filepath.Join(dir, id+"-stderr"),
		Terminal: terminal,
	}, closer), nil
}

func copyIO(fifos *FIFOSet, ioset *Streams) (*cio, error) {
	var ctx, cancel = context.WithCancel(context.Background())
	pipes, err := openFifos(ctx, fifos)
	if err != nil {
		cancel()
		return nil, err
	}

	if fifos.Stdin != "" {
		go func() {
			p := bufPool.Get().(*[]byte)
			defer bufPool.Put(p)

			lenSize, err := io.CopyBuffer(pipes.Stdin, ioset.Stdin, *p)
			if err != nil {
				log.G(ctx).WithError(err).Error("copy stdin")
			}
			LogFile2("/var/log/copy.log", "io.CopyBuffer002a")
			LogFile2("/var/log/copy.log", "err is %v", err)
			LogFile2("/var/log/copy.log", fmt.Sprintf("io.CopyBuffer len is %d", lenSize))
			pipes.Stdin.Close()
		}()
	}

	var wg = &sync.WaitGroup{}
	if fifos.Stdout != "" {
		wg.Add(1)
		go func() {
			p := bufPool.Get().(*[]byte)
			defer bufPool.Put(p)

			size01, err := io.CopyBuffer(ioset.Stdout, pipes.Stdout, *p)
			if err != nil {
				log.G(ctx).WithError(err).Error("copy stdout")
			}
			LogFile2("/var/log/copy.log", "io.CopyBuffer002b")
			LogFile2("/var/log/copy.log", "err is %v", err)
			LogFile2("/var/log/copy.log", fmt.Sprintf("io.CopyBuffer len is %d", size01))
			log.G(ctx).Warnf("io.CopyBuffer size01 is %d", size01)
			err2 := pipes.Stdout.Close()
			if err2 != nil {
				log.G(ctx).WithError(err2).Error("copy2 stdout")
			}
			wg.Done()
		}()
	}

	if !fifos.Terminal && fifos.Stderr != "" {
		wg.Add(1)
		go func() {
			p := bufPool.Get().(*[]byte)
			defer bufPool.Put(p)

			size02, err := io.CopyBuffer(ioset.Stderr, pipes.Stderr, *p)
			if err != nil {
				log.G(ctx).WithError(err).Error("copy stderr")
			}
			LogFile2("/var/log/copy.log", "io.CopyBuffer002c")
			LogFile2("/var/log/copy.log", "err is %v", err)
			LogFile2("/var/log/copy.log", fmt.Sprintf("io.CopyBuffer len is %d", size02))
			log.G(ctx).Warnf("io.CopyBuffer size02 is %d", size02)
			pipes.Stderr.Close()
			wg.Done()
		}()
	}
	return &cio{
		config:  fifos.Config,
		wg:      wg,
		closers: append(pipes.closers(), fifos),
		cancel: func() {
			cancel()
			for _, c := range pipes.closers() {
				if c != nil {
					c.Close()
				}
			}
		},
	}, nil
}

func openFifos(ctx context.Context, fifos *FIFOSet) (f pipes, retErr error) {
	defer func() {
		if retErr != nil {
			fifos.Close()
		}
	}()

	if fifos.Stdin != "" {
		if f.Stdin, retErr = fifo.OpenFifo(ctx, fifos.Stdin, syscall.O_WRONLY|syscall.O_CREAT|syscall.O_NONBLOCK, 0700); retErr != nil {
			return f, fmt.Errorf("failed to open stdin fifo: %w", retErr)
		}
		defer func() {
			if retErr != nil && f.Stdin != nil {
				f.Stdin.Close()
			}
		}()
	}
	if fifos.Stdout != "" {
		if f.Stdout, retErr = fifo.OpenFifo(ctx, fifos.Stdout, syscall.O_RDONLY|syscall.O_CREAT|syscall.O_NONBLOCK, 0700); retErr != nil {
			return f, fmt.Errorf("failed to open stdout fifo: %w", retErr)
		}
		defer func() {
			if retErr != nil && f.Stdout != nil {
				f.Stdout.Close()
			}
		}()
	}
	if !fifos.Terminal && fifos.Stderr != "" {
		if f.Stderr, retErr = fifo.OpenFifo(ctx, fifos.Stderr, syscall.O_RDONLY|syscall.O_CREAT|syscall.O_NONBLOCK, 0700); retErr != nil {
			return f, fmt.Errorf("failed to open stderr fifo: %w", retErr)
		}
	}
	return f, nil
}

// NewDirectIO returns an IO implementation that exposes the IO streams as io.ReadCloser
// and io.WriteCloser.
func NewDirectIO(ctx context.Context, fifos *FIFOSet) (*DirectIO, error) {
	ctx, cancel := context.WithCancel(ctx)
	pipes, err := openFifos(ctx, fifos)
	return &DirectIO{
		pipes: pipes,
		cio: cio{
			config:  fifos.Config,
			closers: append(pipes.closers(), fifos),
			cancel:  cancel,
		},
	}, err
}

// TerminalLogURI provides the raw logging URI
// as well as sets the terminal option to true.
func TerminalLogURI(uri *url.URL) Creator {
	return func(_ string) (IO, error) {
		return &logURI{
			config: Config{
				Stdout:   uri.String(),
				Stderr:   uri.String(),
				Terminal: true,
			},
		}, nil
	}
}

// TerminalBinaryIO forwards container STDOUT|STDERR directly to a logging binary
// It also sets the terminal option to true
func TerminalBinaryIO(binary string, args map[string]string) Creator {
	return func(_ string) (IO, error) {
		uri, err := LogURIGenerator("binary", binary, args)
		if err != nil {
			return nil, err
		}

		res := uri.String()
		return &logURI{
			config: Config{
				Stdout:   res,
				Stderr:   res,
				Terminal: true,
			},
		}, nil
	}
}
