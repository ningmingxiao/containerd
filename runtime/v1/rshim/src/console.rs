use anyhow::{Context, Result};
use log::{debug, error};
use mio::unix::EventedFd;
use mio::unix::UnixReady;
use mio::{Events, Poll, PollOpt, Ready, Token};
use nix::fcntl::{self, FcntlArg, OFlag};
use rust_runc::io_linux::{Io, PipeIO};
use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};
use std::{
    io::{Read, Write},
    os::unix::io::{AsRawFd, RawFd},
};

pub fn set_non_blocking(fd: RawFd, nonblocking: bool) -> Result<()> {
    let flag_bits = fcntl::fcntl(fd, FcntlArg::F_GETFL)?;
    let mut flag = OFlag::from_bits_truncate(flag_bits);
    flag.set(OFlag::O_NONBLOCK, nonblocking);
    fcntl::fcntl(fd, FcntlArg::F_SETFL(flag))?;
    debug!("set flag: {:?}", flag);

    Ok(())
}

//TODO: tmp fix
//https://i.zte.com.cn/#/space/5dfdcad1d15c4fd1970c80de31256313/wiki/page/467b2b8c133c48e6a5573628e939f4f8/view
const BUFFER_SIZE: usize = 2048;

fn copyio(reader: &mut std::fs::File, writer: &mut std::fs::File) -> Result<()> {
    let mut buffer = [0; BUFFER_SIZE];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                match writer.write_all(&buffer[0..n]) {
                    Ok(()) => {}
                    Err(e) => {
                        error!("write failed: {:?}", e);
                        return Err(e)?;
                    }
                }
                if n < buffer.len() {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                error!("copyio failed: {:?}", e);
                return Err(e)?;
            }
        }
    }

    Ok(())
}

const CONSOLE: Token = Token(0);
const STDIN: Token = Token(1);
const STDOUT: Token = Token(2);
const STDERR: Token = Token(3);

struct _ConsoleIO<'a> {
    reader: &'a mut std::fs::File,
    writer: &'a mut std::fs::File,
}

pub fn copy_pipes(pipe_io: &PipeIO, stdin: &str, stdout: &str, stderr: &str) -> Result<()> {
    let poll = Poll::new().context("failed to new poll")?;
    let mut events = Events::with_capacity(8);

    let mut sin: Option<std::fs::File> = None;
    if stdin != "" {
        let stdin_file = OpenOptions::new()
            .read(true)
            .write(false)
            .custom_flags(libc::O_NONBLOCK)
            .open(stdin)
            .context("failed to open stdin")?;

        poll.register(
            &EventedFd(&stdin_file.as_raw_fd()),
            STDIN,
            Ready::readable() | UnixReady::hup(),
            PollOpt::edge(),
        )
        .context("failed to register stdin")?;

        sin = Some(stdin_file);
    }

    if let Some(ref stdout) = pipe_io.stdout {
        poll.register(
            &EventedFd(&stdout.r),
            STDOUT,
            Ready::readable() | UnixReady::hup(),
            PollOpt::edge(),
        )
        .context("failed to register stdout")?;
    }

    if let Some(ref stderr) = pipe_io.stderr {
        poll.register(
            &EventedFd(&stderr.r),
            STDERR,
            Ready::readable() | UnixReady::hup(),
            PollOpt::edge(),
        )
        .context("failed to register stderr")?;
    }

    let mut console_in = pipe_io.stdin();

    let mut console_out = pipe_io.stdout();

    let mut console_err = pipe_io.stderr();

    let mut sout = Some(
        OpenOptions::new()
            .write(true)
            .create(false)
            .open(stdout)
            .context("failed to open stdout")?,
    );
    let mut serr = Some(
        OpenOptions::new()
            .write(true)
            .create(false)
            .open(stderr)
            .context("failed to open stderr")?,
    );

    loop {
        poll.poll(&mut events, None).context("failed to poll")?;
        for event in &events {
            match event.token() {
                STDIN => {
                    if let Some(mut i) = sin.as_mut() {
                        if let Some(ref mut c) = console_in {
                            copyio(&mut i, c).context("failed to copyio for stdin")?;
                            if event.readiness().contains(UnixReady::hup()) {
                                if let Some(f) = console_in.take() {
                                    drop(f);
                                }
                            }
                        }
                    }
                }

                STDOUT => {
                    if let Some(ref mut c) = console_out {
                        if let Some(ref mut o) = sout {
                            copyio(c, o).context("failed to copyio for stdout")?;
                            if event.readiness().contains(UnixReady::hup()) {
                                if let Some(o) = sout.take() {
                                    drop(o);
                                }
                                if let Some(f) = console_out.take() {
                                    drop(f);
                                }
                                // TODO: 上面的流程已经可以保证通知docker端， 流程中断， 可以退出了。
                                // 这里就不需要使用返回错误的形式处理了， 后续可以考虑将所有IO放到一个
                                // 固定线程中
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "EOF",
                                ))?;
                            }
                        }
                    }
                }

                STDERR => {
                    if let Some(ref mut c) = console_err {
                        if let Some(ref mut e) = serr {
                            copyio(c, e).context("failed to copyio for stderr")?;
                            if event.readiness().contains(UnixReady::hup()) {
                                error!("stderr hup!");
                                if let Some(e) = serr.take() {
                                    drop(e);
                                }
                                if let Some(f) = console_err.take() {
                                    drop(f);
                                }
                                // TODO: 上面的流程已经可以保证通知docker端， 流程中断， 可以退出了。
                                // 这里就不需要使用返回错误的形式处理了， 后续可以考虑将所有IO放到一个
                                // 固定线程中
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "EOF",
                                ))?;
                            }
                        }
                    }
                }

                Token(_) => {
                    error!("unexpected event: {:?}", event);
                }
            }
        }
    }
}

pub fn copy_console(console: &mut std::fs::File, stdin: &str, stdout: &str) -> Result<()> {
    let poll = Poll::new().context("failed to new poll")?;
    let mut events = Events::with_capacity(4);

    let mut sin: Option<std::fs::File> = None;
    if stdin != "" {
        let stdin_file = OpenOptions::new()
            .read(true)
            .write(false)
            .custom_flags(libc::O_NONBLOCK)
            .open(stdin)
            .context("failed to open stdin")?;

        poll.register(
            &EventedFd(&stdin_file.as_raw_fd()),
            STDIN,
            Ready::readable(),
            PollOpt::edge(),
        )?;

        sin = Some(stdin_file);
    }

    let mut sout = OpenOptions::new()
        .write(true)
        .open(stdout)
        .context("failed to open stdout")?;

    let fd = console.as_raw_fd();
    let console_eventfd = EventedFd(&fd);
    poll.register(
        &console_eventfd,
        CONSOLE,
        Ready::readable() | UnixReady::hup(),
        PollOpt::edge(),
    )
    .context("failed to register console")?;

    set_non_blocking(console.as_raw_fd(), true).context("failed to set_non_blocking")?;

    loop {
        poll.poll(&mut events, None).context("failed to poll")?;
        for event in &events {
            match event.token() {
                CONSOLE => {
                    copyio(console, &mut sout).context("failed to copyio for console")?;
                    if event.readiness().contains(UnixReady::hup()) {
                        error!("pty console hup!");
                        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "EOF"))?;
                    }
                }
                token => {
                    if token == STDIN {
                        if let Some(mut i) = sin.as_mut() {
                            copyio(&mut i, console).context("failed to copyio for token")?;
                        }
                    } else {
                        error!("unhandle events: {:?}", event);
                    };
                }
            }
        }
    }
}
