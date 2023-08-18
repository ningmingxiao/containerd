// Copyright (c) 2020 Klaus Post, released under MIT License. See LICENSE file.

//go:build arm64 && !linux && !darwin
// +build arm64,!linux,!darwin

package cpuid

func detectOS(c *CPUInfo) bool {
	return false
}
