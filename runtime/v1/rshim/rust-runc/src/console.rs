/*
 * Copyright 2019 fsyncd, Berlin, Germany.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::alloc::{alloc, Layout};
use std::ffi::c_void;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::FromRawFd;
use std::path::PathBuf;
use std::{fs, ptr};

use log::warn;

use crate::*;

/// Receive a PTY master over the provided unix socket
pub struct ReceivePtyMaster {
    console_socket: PathBuf,
    listener: Option<std::os::unix::net::UnixListener>,
}

// Looks to be a false positive
#[allow(clippy::cast_ptr_alignment)]
impl ReceivePtyMaster {
    /// Bind a unix domain socket to the provided path
    pub fn new(console_socket: &PathBuf) -> Result<Self, Error> {
        let listener = std::os::unix::net::UnixListener::bind(console_socket)
            .context(UnixSocketOpenError {})?;
        Ok(Self {
            console_socket: console_socket.clone(),
            listener: Some(listener),
        })
    }

    /// Receive a master PTY file descriptor from the socket
    pub fn receive(mut self) -> Result<std::fs::File, Error> {
        let io = self.listener.take().unwrap();
        let (console_stream, _) = io.accept().unwrap();
        loop {
            {
                // 4096 is the max name length from the go-runc implementation
                let mut iov_base = [0u8; 4096];
                let mut message_buf = [0u8; 24];
                let mut io = libc::iovec {
                    iov_len: iov_base.len(),
                    iov_base: &mut iov_base as *mut _ as *mut c_void,
                };

                let mut msg: Box<libc::msghdr> = unsafe {
                    Box::from_raw(alloc(Layout::new::<libc::msghdr>()) as *mut libc::msghdr)
                };

                #[cfg(target_env = "gnu")]
                type BufSize = usize;

                #[cfg(target_env = "musl")]
                type BufSize = u32;

                msg.msg_name = ptr::null_mut();
                msg.msg_namelen = 0;
                msg.msg_iov = &mut io;
                msg.msg_iovlen = 1;
                msg.msg_control = &mut message_buf as *mut _ as *mut c_void;
                msg.msg_controllen = message_buf.len() as BufSize;
                msg.msg_flags = 0;

                let console_stream_fd = console_stream.as_raw_fd();
                let ret = unsafe { libc::recvmsg(console_stream_fd, &mut *msg, 0) };
                ensure!(ret >= 0, UnixSocketReceiveMessageError {});
                unsafe {
                    let cmsg = libc::CMSG_FIRSTHDR(&*msg);
                    if cmsg.is_null() {
                        continue;
                    }
                    let cmsg_data = libc::CMSG_DATA(cmsg);
                    ensure!(!cmsg_data.is_null(), UnixSocketReceiveMessageError {});
                    return Ok(std::fs::File::from_raw_fd(ptr::read_unaligned(
                        cmsg_data as *const i32,
                    )));
                }
            }
        }
    }
}

impl Drop for ReceivePtyMaster {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.console_socket) {
            warn!("failed to clean up console socket: {}", e);
        }
    }
}
