#![allow(non_snake_case)]

use libc::{self, c_char};
use log::{error, info};
use std::collections::HashMap;
use std::ffi::CString;
use std::fmt;
use std::mem;
use std::path::Path;
use std::{thread, time};

#[derive(Hash, Eq, PartialEq, Debug)]
struct MountPair {
    clear: bool,
    flag: u64,
}

impl MountPair {
    pub fn new(clear: bool, flag: u64) -> MountPair {
        return MountPair {
            clear: clear,
            flag: flag,
        };
    }
}

#[derive(PartialEq, Clone, Default, Debug)]
pub struct Mount {
    pub filed_type: String,
    pub source: String,
    pub options: Vec<String>,
}

impl Mount {
    pub fn new(filed_type: String, source: String, options: Vec<String>) -> Self {
        Mount {
            filed_type,
            source,
            options,
        }
    }
    pub fn mount(&self, target: String) -> i32 {
        let mut ret = 0;

        let pagesize;
        unsafe {
            pagesize = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        }

        let mut cdir = "".to_string();
        let mut options: Vec<String> = Vec::new();

        // avoid hitting one page limit of mount argument buffer
        // NOTE: 512 is a buffer during pagesize check.
        let buffer = 512;
        if self.filed_type == "overlay".to_string()
            && options_size(&self.options) >= pagesize - buffer
        {
            let (dir, opts) = compact_lowerdir_option(&self.options);
            cdir = dir;
            options = opts;
        }

        let mount_data;
        if cdir != "".to_string() {
            mount_data = parse_mount_options(&options);
        } else {
            mount_data = parse_mount_options(&self.options);
        }

        let (flags, data) = mount_data;

        info!("flags {:?}, data {:?}", flags, data);

        if data.len() > pagesize {
            error!("mount options is too long");
            return -1;
        }

        let ptypes = libc::MS_SHARED | libc::MS_PRIVATE | libc::MS_SLAVE | libc::MS_UNBINDABLE;

        let oflags = flags & !ptypes;

        let tar = match CString::new(target.clone()) {
            Ok(t) => t,
            Err(error) => {
                error!("CString::new target failed in {:?}!", error);
                return -1;
            }
        };

        if flags & libc::MS_REMOUNT == 0 || data != "".to_string() {
            let source = match CString::new(self.source.as_str()) {
                Ok(s) => s,
                Err(error) => {
                    error!("CString::new source failed in {:?}!", error);
                    return -1;
                }
            };
            let ftype = match CString::new(self.filed_type.as_str()) {
                Ok(f) => f,
                Err(error) => {
                    error!("CString::new ftype failedin {:?}!", error);
                    return -1;
                }
            };
            let data_str = match CString::new(data.as_str()) {
                Ok(d) => d,
                Err(error) => {
                    error!("CString::new data_str failed in {:?}!", error);
                    return -1;
                }
            };
            info!("the mount source: {:?}, the mount target: {:?}, the mount type: {:?}, the mount data: {:?}", source, tar, ftype, data_str);
            let data_ptr = data_str.as_ptr() as *const libc::c_void;
            if cdir == "".to_string() {
                unsafe {
                    ret = libc::mount(
                        source.as_ptr(),
                        tar.as_ptr(),
                        ftype.as_ptr(),
                        oflags,
                        data_ptr,
                    );
                }
            } else {
                unsafe {
                    let mut buf = Vec::with_capacity(512);
                    let ptr = buf.as_mut_ptr() as *mut c_char;
                    let pwd = libc::getcwd(ptr, buf.capacity());
                    if pwd.is_null() {
                        error!("getcwd error!");
                        return -1;
                    }
                    let path = match CString::new(cdir.clone()) {
                        Ok(p) => p,
                        Err(error) => {
                            error!("CString::new path failed in {:?}!", error);
                            return -1;
                        }
                    };
                    let mut dst = mem::MaybeUninit::uninit();
                    ret = libc::stat(path.as_ptr(), dst.as_mut_ptr());
                    if ret != 0 {
                        error!("stat {} failed!", cdir.clone());
                        return ret;
                    }

                    ret = libc::chdir(path.as_ptr());
                    if ret != 0 {
                        error!("chdir {} failed!", cdir.clone());
                        return ret;
                    }
                    ret = libc::mount(
                        source.as_ptr(),
                        tar.as_ptr(),
                        ftype.as_ptr(),
                        oflags,
                        data_ptr,
                    );
                    if ret != 0 {
                        error!("mount {} failed!", cdir.clone());
                        return ret;
                    }
                    ret = libc::chdir(pwd);
                    if ret != 0 {
                        error!("chdir {:?} failed!", pwd);
                        return ret;
                    }
                }
            }
        }

        if flags & ptypes != 0 {
            let pflags = ptypes | libc::MS_REC | libc::MS_SILENT;
            let null = CString::new("").expect("CString::new null failed");

            unsafe {
                ret = libc::mount(
                    null.as_ptr(),
                    tar.as_ptr(),
                    null.as_ptr(),
                    flags & pflags,
                    null.as_ptr() as *const libc::c_void,
                );
            }
        }

        let broflags = libc::MS_BIND | libc::MS_RDONLY;

        if oflags & broflags == broflags {
            let null = CString::new("").expect("CString::new null failed");
            unsafe {
                ret = libc::mount(
                    null.as_ptr(),
                    tar.as_ptr(),
                    null.as_ptr(),
                    oflags & libc::MS_REMOUNT,
                    null.as_ptr() as *const libc::c_void,
                );
            }
        }

        return ret;
    }
}

pub fn umount(target: String) -> i32 {
    let mut ret = 0;
    let mut counter = 0;
    while counter < 50 {
        let real_target = CString::new(target.as_str()).expect("CString::new source failed");
        unsafe {
            ret = libc::umount(real_target.as_ptr());
            if ret == libc::EBUSY {
                let time_out = time::Duration::from_millis(50);
                thread::sleep(time_out);
                counter += 1;
                continue;
            } else {
                break;
            }
        }
    }

    return ret;
}

pub fn umount_all(target: String) -> i32 {
    let ret;
    loop {
        ret = umount(target.clone());
        if ret == libc::EINVAL {
            return 0;
        }
        return ret;
    }
}

pub fn options_size(opts: &Vec<String>) -> usize {
    let mut size = 0;
    for opt in opts {
        size += opt.len();
    }
    info!("the total size is {:?}", size);
    return size;
}

pub fn compact_lowerdir_option(options: &Vec<String>) -> (String, Vec<String>) {
    let mut vstr: Vec<String> = Vec::new();
    let (idx, dirs) = find_overlay_lower_dirs(options);

    info!("idx: {:?}, dirs: {:?}", idx, dirs);

    if idx == -1 || dirs.len() == 1 {
        let mut vstr: Vec<String> = Vec::new();
        vstr.push(options[0].clone());
        return ("".to_string(), vstr);
    }

    let mut commondir = longest_common_prefix(&dirs);

    info!("commondir: {:?}", commondir);

    if commondir == "".to_string() {
        for opt in options {
            vstr.push(opt.clone());
        }
        return ("".to_string(), vstr);
    }

    let parent = Path::new(&commondir).parent();
    let path = match parent {
        None => return ("".to_string(), vstr),
        Some(p) => p,
    };

    let parentdir = match path.to_str() {
        None => return ("".to_string(), vstr),
        Some(t) => t,
    };

    commondir = parentdir.to_string();

    if commondir == "/".to_string() {
        return ("".to_string(), vstr);
    }

    commondir = commondir + "/";

    let mut newdirs: Vec<String> = Vec::new();

    for dir in dirs {
        newdirs.push(dir[commondir.len()..].to_string());
    }

    let mut index = 0;
    for opt in options {
        if index == idx {
            continue;
        }
        vstr.push(opt.clone());
        index += 1;
    }

    let newdirs_slice = &newdirs[..];

    let lowerdir = fmt::format(format_args!("lowerdir={}", newdirs_slice.join(":")));

    vstr.push(lowerdir);

    info!("commondir: {:?}, vstr: {:?}", commondir, vstr);

    return (commondir, vstr);
}

pub fn find_overlay_lower_dirs(options: &Vec<String>) -> (i32, Vec<String>) {
    let mut idx = -1;

    let mut counter = 0;
    for opt in options {
        if opt.starts_with("lowerdir=") {
            idx = counter;
            break;
        }
        counter += 1;
    }

    let mut opts: Vec<String> = Vec::new();
    if idx == -1 {
        return (-1, opts);
    }

    let lowerdir: Vec<&str> = options[idx as usize]["lowerdir=".len()..]
        .split(":")
        .collect();

    for s in lowerdir {
        opts.push(s.to_string());
    }

    return (idx, opts);
}

pub fn longest_common_prefix(strs: &Vec<String>) -> String {
    if strs.len() == 0 {
        return "".to_string();
    } else if strs.len() == 1 {
        return strs[0].clone();
    }

    let mut min = strs[0].clone();
    let mut max = strs[0].clone();

    for s in strs {
        if min > s.clone() {
            min = s.clone();
        }

        if max < s.clone() {
            max = s.clone();
        }
    }

    let mut index = 0;
    while index < min.len() && index < max.len() {
        if min.chars().nth(index) != max.chars().nth(index) {
            return min[..index].to_string();
        }
        index += 1;
    }

    return min;
}

pub fn parse_mount_options(options: &Vec<String>) -> (u64, String) {
    let mut flag = 0;
    let mut data: Vec<String> = Vec::new();

    let mountnames = vec![
        "async",
        "atime",
        "bind",
        "defaults",
        "dev",
        "diratime",
        "dirsync",
        "exec",
        "mand",
        "noatime",
        "nodev",
        "nodiratime",
        "noexec",
        "nomand",
        "norelatime",
        "nostrictatime",
        "nosuid",
        "rbind",
        "relatime",
        "remount",
        "ro",
        "rw",
        "strictatime",
        "suid",
        "sync",
    ];

    let mountpairs = vec![
        MountPair::new(true, libc::MS_SYNCHRONOUS),
        MountPair::new(true, libc::MS_NOATIME),
        MountPair::new(false, libc::MS_BIND),
        MountPair::new(false, 0),
        MountPair::new(true, libc::MS_NODEV),
        MountPair::new(true, libc::MS_NODIRATIME),
        MountPair::new(false, libc::MS_DIRSYNC),
        MountPair::new(true, libc::MS_NOEXEC),
        MountPair::new(false, libc::MS_MANDLOCK),
        MountPair::new(false, libc::MS_NOATIME),
        MountPair::new(false, libc::MS_NODEV),
        MountPair::new(false, libc::MS_NODIRATIME),
        MountPair::new(false, libc::MS_NOEXEC),
        MountPair::new(true, libc::MS_MANDLOCK),
        MountPair::new(true, libc::MS_RELATIME),
        MountPair::new(true, libc::MS_STRICTATIME),
        MountPair::new(false, libc::MS_NOSUID),
        MountPair::new(false, libc::MS_BIND | libc::MS_REC),
        MountPair::new(false, libc::MS_RELATIME),
        MountPair::new(false, libc::MS_REMOUNT),
        MountPair::new(false, libc::MS_RDONLY),
        MountPair::new(true, libc::MS_RDONLY),
        MountPair::new(false, libc::MS_STRICTATIME),
        MountPair::new(true, libc::MS_NOSUID),
        MountPair::new(false, libc::MS_SYNCHRONOUS),
    ];

    let mountflags: HashMap<_, _> = mountnames.iter().zip(mountpairs.iter()).collect();

    info!("options {:?}", options);
    for opt in options {
        info!("opt {:?}", opt);
        if mountflags.contains_key(&opt.as_str()) && mountflags[&opt.as_str()].flag != 0 {
            if mountflags[&opt.as_str()].clear {
                flag &= !mountflags[&opt.as_str()].flag;
            } else {
                flag |= mountflags[&opt.as_str()].flag;
            }
        } else {
            data.push(opt.to_string());
        }
    }

    return (flag, data[..].join(","));
}
