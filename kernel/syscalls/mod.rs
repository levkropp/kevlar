// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    ctypes::*,
    debug::{self, DebugEvent, DebugFilter},
    fs::path::PathBuf,
    fs::{
        opened_file::{Fd, OpenFlags},
        path::Path,
        stat::FileMode,
    },
    net::{RecvFromFlags, SendToFlags},
    process::{current_process, process_group::PgId, PId, Process},
    result::{Errno, Error, Result},
    syscalls::{getrandom::GetRandomFlags, wait4::WaitOptions},
    timer::Timeval,
    user_buffer::UserCStr,
};
use bitflags::bitflags;
use kevlar_platform::{address::UserVAddr, arch::PtRegs};

mod accept;
mod arch_prctl;
mod bind;
mod brk;
mod chdir;
mod chmod;
mod clock_gettime;
mod close;
mod connect;
mod dup2;
mod execve;
mod exit;
mod exit_group;
mod fcntl;
mod fork;
mod fstat;
mod fsync;
mod getcwd;
mod getdents64;
mod getpeername;
mod getpriority;
mod getpgid;
mod getpid;
mod getppid;
mod getrandom;
mod getsockname;
mod getsockopt;
mod gettid;
mod ioctl;
mod kill;
mod link;
mod linkat;
mod listen;
mod lstat;
mod mkdir;
mod mmap;
mod open;
mod pipe;
mod poll;
mod read;
mod readlink;
mod reboot;
mod recvfrom;
mod rt_sigaction;
mod rt_sigprocmask;
mod rt_sigreturn;
mod select;
mod sendto;
mod set_tid_address;
mod setpgid;
mod shutdown;
mod socket;
mod stat;
mod syslog;
mod uname;
mod utimes;
mod vfork;
mod wait4;
mod write;
mod writev;

// M1 Phase 1: Trivial syscalls
mod dup;
mod getegid;
mod getpgrp;
mod sched_getaffinity;
mod sched_yield;
mod umask;

// M1 Phase 2: FD plumbing
mod dup3;
mod pipe2;

// M1 Phase 3: *at syscalls + file ops
mod access;
mod lseek;
mod newfstatat;
mod openat;

// M1 Phase 4: Filesystem mutations
mod rename;
mod rmdir;
mod unlink;

// M1 Phase 5: Time & system info
mod getrlimit;
mod gettimeofday;
mod nanosleep;
mod sysinfo;

// M1 Phase 6: Memory management
mod mprotect;
mod munmap;

// M2: Dynamic linking
pub mod futex;
mod madvise;
mod pread64;
mod set_robust_list;

// M3: Terminal control, session management, *at syscalls, file ops
mod fchdir;
mod ftruncate;
mod getrusage;
mod mkdirat;
mod pwrite64;
mod readlinkat;
mod readv;
mod renameat;
mod getsid;
mod setsid;
mod sigaltstack;
mod symlinkat;
mod unlinkat;

// M3 Phase 5: Job control, clone, additional stubs
mod alarm;
pub(crate) mod setitimer;
mod clone;
mod fchmod;
mod getgroups;
mod pause;
mod rt_sigsuspend;
mod tgkill;

// M4: systemd support
mod capability;
mod epoll;
mod eventfd;
mod mount;
mod prctl;
mod recvmsg;
mod sendmsg;
mod setsockopt;
mod signalfd;
mod timerfd;

// M5 Phase 1: File metadata & extended operations
mod fadvise64;
mod fallocate;
mod preadv;
mod statfs;
mod statx;
mod utimensat;

// M5 Phase 2: inotify
mod inotify;

// M5 Phase 3: Zero-copy I/O
mod sendfile;
mod splice;

// M7 Phase 6: glibc syscall stubs
mod clone3;
mod rseq;
mod sched_scheduler;
mod sched_setaffinity;

// M8 Phase 2: Namespaces
mod sethostname;
mod unshare;

// M8 Phase 3: Filesystem isolation
mod pivot_root;

// M9 Phase 1: Syscall gap closure
mod close_range;
mod flock;
mod memfd_create;
mod name_to_handle_at;
mod pidfd_open;
mod waitid;

pub enum CwdOrFd {
    /// `AT_FDCWD`
    AtCwd,
    Fd(Fd),
}

impl CwdOrFd {
    pub fn parse(value: c_int) -> CwdOrFd {
        match value {
            -100 => CwdOrFd::AtCwd,
            _ => CwdOrFd::Fd(Fd::new(value)),
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AtFlags: c_int {
        const AT_SYMLINK_FOLLOW = 0x400;
    }
}

const MAX_READ_WRITE_LEN: usize = core::isize::MAX as usize;
const IOV_MAX: usize = 1024;

#[repr(C)]
struct IoVec {
    base: UserVAddr,
    len: usize,
}

// x86_64 syscall numbers.
#[cfg(target_arch = "x86_64")]
mod syscall_numbers {
    pub const SYS_READ: usize = 0;
    pub const SYS_WRITE: usize = 1;
    pub const SYS_OPEN: usize = 2;
    pub const SYS_CLOSE: usize = 3;
    pub const SYS_STAT: usize = 4;
    pub const SYS_FSTAT: usize = 5;
    pub const SYS_LSTAT: usize = 6;
    pub const SYS_POLL: usize = 7;
    pub const SYS_LSEEK: usize = 8;
    pub const SYS_MMAP: usize = 9;
    pub const SYS_MPROTECT: usize = 10;
    pub const SYS_MUNMAP: usize = 11;
    pub const SYS_BRK: usize = 12;
    pub const SYS_RT_SIGACTION: usize = 13;
    pub const SYS_RT_SIGPROCMASK: usize = 14;
    pub const SYS_RT_SIGRETURN: usize = 15;
    pub const SYS_PREAD64: usize = 17;
    pub const SYS_IOCTL: usize = 16;
    pub const SYS_WRITEV: usize = 20;
    pub const SYS_MADVISE: usize = 28;
    pub const SYS_ACCESS: usize = 21;
    pub const SYS_PIPE: usize = 22;
    pub const SYS_SELECT: usize = 23;
    pub const SYS_SCHED_YIELD: usize = 24;
    pub const SYS_DUP: usize = 32;
    pub const SYS_DUP2: usize = 33;
    pub const SYS_NANOSLEEP: usize = 35;
    pub const SYS_GETPID: usize = 39;
    pub const SYS_SOCKET: usize = 41;
    pub const SYS_CONNECT: usize = 42;
    pub const SYS_ACCEPT: usize = 43;
    pub const SYS_SENDTO: usize = 44;
    pub const SYS_RECVFROM: usize = 45;
    pub const SYS_SHUTDOWN: usize = 48;
    pub const SYS_BIND: usize = 49;
    pub const SYS_LISTEN: usize = 50;
    pub const SYS_GETSOCKNAME: usize = 51;
    pub const SYS_GETPEERNAME: usize = 52;
    pub const SYS_SOCKETPAIR: usize = 53;
    pub const SYS_GETSOCKOPT: usize = 55;
    pub const SYS_FORK: usize = 57;
    pub const SYS_VFORK: usize = 58;
    pub const SYS_EXECVE: usize = 59;
    pub const SYS_EXIT: usize = 60;
    pub const SYS_WAIT4: usize = 61;
    pub const SYS_KILL: usize = 62;
    pub const SYS_UNAME: usize = 63;
    pub const SYS_FCNTL: usize = 72;
    pub const SYS_FSYNC: usize = 74;
    pub const SYS_GETCWD: usize = 79;
    pub const SYS_CHDIR: usize = 80;
    pub const SYS_RENAME: usize = 82;
    pub const SYS_MKDIR: usize = 83;
    pub const SYS_RMDIR: usize = 84;
    pub const SYS_LINK: usize = 86;
    pub const SYS_UNLINK: usize = 87;
    pub const SYS_READLINK: usize = 89;
    pub const SYS_CHMOD: usize = 90;
    pub const SYS_CHOWN: usize = 92;
    pub const SYS_UMASK: usize = 95;
    pub const SYS_GETTIMEOFDAY: usize = 96;
    pub const SYS_GETRLIMIT: usize = 97;
    pub const SYS_SYSINFO: usize = 99;
    pub const SYS_GETUID: usize = 102;
    pub const SYS_SYSLOG: usize = 103;
    pub const SYS_GETGID: usize = 104;
    pub const SYS_SETUID: usize = 105;
    pub const SYS_SETGID: usize = 106;
    pub const SYS_GETEUID: usize = 107;
    pub const SYS_GETEGID: usize = 108;
    pub const SYS_SETPGID: usize = 109;
    pub const SYS_GETPPID: usize = 110;
    pub const SYS_GETPGRP: usize = 111;
    pub const SYS_SETSID: usize = 112;
    pub const SYS_SETGROUPS: usize = 116;
    pub const SYS_GETPGID: usize = 121;
    pub const SYS_FCHDIR: usize = 81;
    pub const SYS_FTRUNCATE: usize = 77;
    pub const SYS_GETRUSAGE: usize = 98;
    pub const SYS_READV: usize = 19;
    pub const SYS_PWRITE64: usize = 18;
    pub const SYS_READLINKAT: usize = 267;
    pub const SYS_UNLINKAT: usize = 263;
    pub const SYS_MKDIRAT: usize = 258;
    pub const SYS_RENAMEAT: usize = 264;
    pub const SYS_RENAMEAT2: usize = 316;
    pub const SYS_SYMLINK: usize = 88;
    pub const SYS_SYMLINKAT: usize = 266;
    pub const SYS_GETSID: usize = 124;
    pub const SYS_SIGALTSTACK: usize = 131;
    pub const SYS_ARCH_PRCTL: usize = 158;
    pub const SYS_REBOOT: usize = 169;
    pub const SYS_GETTID: usize = 186;
    pub const SYS_GETDENTS64: usize = 217;
    pub const SYS_SET_TID_ADDRESS: usize = 218;
    pub const SYS_CLOCK_GETTIME: usize = 228;
    pub const SYS_EXIT_GROUP: usize = 231;
    pub const SYS_UTIMES: usize = 235;
    pub const SYS_OPENAT: usize = 257;
    pub const SYS_NEWFSTATAT: usize = 262;
    pub const SYS_LINKAT: usize = 265;
    pub const SYS_DUP3: usize = 292;
    pub const SYS_PIPE2: usize = 293;
    pub const SYS_CLONE: usize = 56;
    pub const SYS_FACCESSAT: usize = 269;
    pub const SYS_PPOLL: usize = 271;
    pub const SYS_PRLIMIT64: usize = 302;
    pub const SYS_FCHMODAT: usize = 268;
    pub const SYS_FCHOWNAT: usize = 260;
    pub const SYS_FUTEX: usize = 202;
    pub const SYS_SCHED_GETAFFINITY: usize = 204;
    pub const SYS_SET_ROBUST_LIST: usize = 273;
    pub const SYS_GETRANDOM: usize = 318;
    pub const SYS_TKILL: usize = 200;
    pub const SYS_TGKILL: usize = 234;
    pub const SYS_RT_SIGSUSPEND: usize = 130;
    pub const SYS_FCHMOD: usize = 91;
    pub const SYS_FCHOWN: usize = 93;
    pub const SYS_PAUSE: usize = 34;
    pub const SYS_SETITIMER: usize = 38;
    pub const SYS_ALARM: usize = 37;
    pub const SYS_GETGROUPS: usize = 115;
    pub const SYS_SENDMSG: usize = 46;
    pub const SYS_RECVMSG: usize = 47;
    pub const SYS_SETSOCKOPT: usize = 54;
    pub const SYS_CAPGET: usize = 125;
    pub const SYS_CAPSET: usize = 126;
    pub const SYS_PRCTL: usize = 157;
    pub const SYS_MOUNT: usize = 165;
    pub const SYS_UMOUNT2: usize = 166;
    pub const SYS_ACCEPT4: usize = 288;
    // M4: epoll + event fds
    pub const SYS_EPOLL_WAIT: usize = 232;
    pub const SYS_EPOLL_CTL: usize = 233;
    pub const SYS_EPOLL_PWAIT: usize = 281;
    pub const SYS_TIMERFD_CREATE: usize = 283;
    pub const SYS_TIMERFD_SETTIME: usize = 286;
    pub const SYS_SIGNALFD4: usize = 289;
    pub const SYS_EVENTFD2: usize = 290;
    pub const SYS_EPOLL_CREATE1: usize = 291;
    pub const SYS_GETPRIORITY: usize = 140;
    pub const SYS_SETPRIORITY: usize = 141;
    // M5 Phase 1: File metadata & extended operations
    pub const SYS_STATFS: usize = 137;
    pub const SYS_FSTATFS: usize = 138;
    pub const SYS_FADVISE64: usize = 221;
    pub const SYS_UTIMENSAT: usize = 280;
    pub const SYS_FALLOCATE: usize = 285;
    pub const SYS_PREADV: usize = 295;
    pub const SYS_PWRITEV: usize = 296;
    pub const SYS_STATX: usize = 332;
    // M5 Phase 2: inotify
    pub const SYS_INOTIFY_ADD_WATCH: usize = 254;
    pub const SYS_INOTIFY_RM_WATCH: usize = 255;
    pub const SYS_INOTIFY_INIT1: usize = 294;
    // M5 Phase 3: Zero-copy I/O
    pub const SYS_SENDFILE: usize = 40;
    pub const SYS_SPLICE: usize = 275;
    pub const SYS_TEE: usize = 276;
    pub const SYS_COPY_FILE_RANGE: usize = 326;
    // M7 Phase 6: glibc syscall stubs
    pub const SYS_SCHED_SETSCHEDULER: usize = 144;
    pub const SYS_SCHED_GETSCHEDULER: usize = 145;
    pub const SYS_SCHED_SETAFFINITY: usize = 203;
    pub const SYS_RSEQ: usize = 334;
    pub const SYS_CLONE3: usize = 435;
    // M8 Phase 2: Namespaces
    pub const SYS_SETHOSTNAME: usize = 170;
    pub const SYS_SETDOMAINNAME: usize = 171;
    pub const SYS_UNSHARE: usize = 272;
    // M8 Phase 3: Filesystem isolation
    pub const SYS_PIVOT_ROOT: usize = 155;
    // M9 Phase 1: Syscall gap closure
    pub const SYS_FLOCK: usize = 73;
    pub const SYS_WAITID: usize = 247;
    pub const SYS_MEMFD_CREATE: usize = 319;
    pub const SYS_MKNOD: usize = 133;
    pub const SYS_SETTIMEOFDAY: usize = 164;
    pub const SYS_CLOCK_SETTIME: usize = 227;
    pub const SYS_NAME_TO_HANDLE_AT: usize = 303;
    pub const SYS_PIDFD_OPEN: usize = 434;
    pub const SYS_CLOSE_RANGE: usize = 436;
}

// ARM64 (AArch64) syscall numbers from asm-generic/unistd.h.
#[cfg(target_arch = "aarch64")]
mod syscall_numbers {
    pub const SYS_GETCWD: usize = 17;
    pub const SYS_DUP: usize = 23;
    pub const SYS_DUP3: usize = 24;
    pub const SYS_FCNTL: usize = 25;
    pub const SYS_IOCTL: usize = 29;
    pub const SYS_LINKAT: usize = 37;
    // ARM64 doesn't have these old syscalls natively. Use unique dummy
    // values so the match arms compile but will never be reached.
    pub const SYS_UNLINK: usize = 0xF001;
    pub const SYS_LINK: usize = 0xF002;
    pub const SYS_MKDIR: usize = 0xF003;
    pub const SYS_RMDIR: usize = 0xF004;
    pub const SYS_CHMOD: usize = 0xF005;
    pub const SYS_CHOWN: usize = 0xF006;
    pub const SYS_RENAME: usize = 0xF007;
    pub const SYS_READLINK: usize = 0xF008;
    pub const SYS_STAT: usize = 0xF009;
    pub const SYS_LSTAT: usize = 0xF00A;
    pub const SYS_ACCESS: usize = 0xF00B;
    pub const SYS_OPEN: usize = 0xF00C;
    pub const SYS_PIPE: usize = 0xF00D;
    pub const SYS_DUP2: usize = 0xF00E;
    pub const SYS_MKDIRAT: usize = 34;
    pub const SYS_UNLINKAT: usize = 35;
    pub const SYS_RENAMEAT: usize = 38;
    pub const SYS_UMASK: usize = 166;
    pub const SYS_FSYNC: usize = 82;
    pub const SYS_CHDIR: usize = 49;
    pub const SYS_FSTAT: usize = 80;
    pub const SYS_NEWFSTATAT: usize = 79;
    pub const SYS_OPENAT: usize = 56;
    pub const SYS_CLOSE: usize = 57;
    pub const SYS_PIPE2: usize = 59;
    pub const SYS_LSEEK: usize = 62;
    pub const SYS_READ: usize = 63;
    pub const SYS_WRITE: usize = 64;
    pub const SYS_WRITEV: usize = 66;
    pub const SYS_SELECT: usize = 1042;  // compat (use pselect6)
    pub const SYS_POLL: usize = 1043;    // compat (use ppoll)
    pub const SYS_READLINKAT: usize = 78;
    pub const SYS_UTIMES: usize = 1037;  // compat
    pub const SYS_GETDENTS64: usize = 61;
    pub const SYS_MMAP: usize = 222;
    pub const SYS_MPROTECT: usize = 226;
    pub const SYS_MUNMAP: usize = 215;
    pub const SYS_BRK: usize = 214;
    pub const SYS_SCHED_GETAFFINITY: usize = 123;
    pub const SYS_SCHED_YIELD: usize = 124;
    pub const SYS_NANOSLEEP: usize = 101;
    pub const SYS_GETTIMEOFDAY: usize = 169;
    pub const SYS_GETRLIMIT: usize = 163;  // prlimit64 is 261
    pub const SYS_SYSINFO: usize = 179;
    pub const SYS_GETPID: usize = 172;
    pub const SYS_GETPPID: usize = 173;
    pub const SYS_GETUID: usize = 174;
    pub const SYS_GETEUID: usize = 175;
    pub const SYS_GETGID: usize = 176;
    pub const SYS_GETEGID: usize = 177;
    pub const SYS_GETTID: usize = 178;
    pub const SYS_SOCKET: usize = 198;
    pub const SYS_SOCKETPAIR: usize = 199;
    pub const SYS_BIND: usize = 200;
    pub const SYS_LISTEN: usize = 201;
    pub const SYS_ACCEPT: usize = 202;
    pub const SYS_CONNECT: usize = 203;
    pub const SYS_GETSOCKNAME: usize = 204;
    pub const SYS_GETPEERNAME: usize = 205;
    pub const SYS_SENDTO: usize = 206;
    pub const SYS_RECVFROM: usize = 207;
    pub const SYS_GETSOCKOPT: usize = 209;
    pub const SYS_SHUTDOWN: usize = 210;
    pub const SYS_FORK: usize = 1079;    // compat (arm64 uses clone)
    pub const SYS_VFORK: usize = 1071;   // compat
    pub const SYS_EXECVE: usize = 221;
    pub const SYS_EXIT: usize = 93;
    pub const SYS_EXIT_GROUP: usize = 94;
    pub const SYS_WAIT4: usize = 260;
    pub const SYS_KILL: usize = 129;
    pub const SYS_UNAME: usize = 160;
    pub const SYS_SETPGID: usize = 154;
    pub const SYS_GETPGID: usize = 155;
    pub const SYS_SETSID: usize = 157;
    pub const SYS_GETPGRP: usize = 1060; // compat
    pub const SYS_FCHDIR: usize = 50;
    pub const SYS_FTRUNCATE: usize = 46;
    pub const SYS_GETRUSAGE: usize = 165;
    pub const SYS_READV: usize = 65;
    pub const SYS_PWRITE64: usize = 68;
    pub const SYS_RENAMEAT2: usize = 276;
    pub const SYS_SYMLINK: usize = 0xF00F; // compat (arm64 uses symlinkat)
    pub const SYS_SYMLINKAT: usize = 36;
    pub const SYS_GETSID: usize = 156;
    pub const SYS_SIGALTSTACK: usize = 132;
    pub const SYS_SETUID: usize = 146;
    pub const SYS_SETGID: usize = 144;
    pub const SYS_SETGROUPS: usize = 159;
    pub const SYS_SYSLOG: usize = 116;
    pub const SYS_RT_SIGACTION: usize = 134;
    pub const SYS_RT_SIGPROCMASK: usize = 135;
    pub const SYS_RT_SIGRETURN: usize = 139;
    pub const SYS_SET_TID_ADDRESS: usize = 96;
    pub const SYS_CLOCK_GETTIME: usize = 113;
    pub const SYS_GETRANDOM: usize = 278;
    pub const SYS_REBOOT: usize = 142;
    pub const SYS_GETPRIORITY: usize = 141;
    pub const SYS_SETPRIORITY: usize = 140;
    pub const SYS_CLONE: usize = 220;
    pub const SYS_FACCESSAT: usize = 48;
    pub const SYS_PPOLL: usize = 73;
    pub const SYS_PRLIMIT64: usize = 261;
    pub const SYS_FCHMODAT: usize = 53;
    pub const SYS_FCHOWNAT: usize = 55;
    pub const SYS_PREAD64: usize = 67;
    pub const SYS_MADVISE: usize = 233;
    pub const SYS_FUTEX: usize = 98;
    pub const SYS_SET_ROBUST_LIST: usize = 99;
    // ARM64 doesn't have arch_prctl; use a dummy value that won't conflict.
    pub const SYS_ARCH_PRCTL: usize = 0xFFFF;
    pub const SYS_TKILL: usize = 130; // ARM64 tkill
    pub const SYS_TGKILL: usize = 131;
    pub const SYS_RT_SIGSUSPEND: usize = 133;
    // ARM64 only has fchmodat(53)/fchownat(55), not fchmod/fchown.
    pub const SYS_FCHMOD: usize = 0xF010;
    pub const SYS_FCHOWN: usize = 0xF011;
    // ARM64 doesn't have pause/alarm natively.
    pub const SYS_PAUSE: usize = 0xF012;
    pub const SYS_SETITIMER: usize = 103;
    pub const SYS_ALARM: usize = 0xF013;
    pub const SYS_GETGROUPS: usize = 158;
    pub const SYS_SENDMSG: usize = 211;
    pub const SYS_RECVMSG: usize = 212;
    pub const SYS_SETSOCKOPT: usize = 208;
    pub const SYS_MOUNT: usize = 40;
    pub const SYS_UMOUNT2: usize = 39;
    pub const SYS_CAPGET: usize = 90;
    pub const SYS_CAPSET: usize = 91;
    pub const SYS_PRCTL: usize = 167;
    pub const SYS_ACCEPT4: usize = 242;
    // M4: epoll + event fds
    pub const SYS_EPOLL_CREATE1: usize = 20;
    pub const SYS_EPOLL_CTL: usize = 21;
    pub const SYS_EPOLL_PWAIT: usize = 22;
    pub const SYS_EPOLL_WAIT: usize = 0xF014; // ARM64 doesn't have old epoll_wait
    pub const SYS_TIMERFD_CREATE: usize = 85;
    pub const SYS_TIMERFD_SETTIME: usize = 86;
    pub const SYS_SIGNALFD4: usize = 74;
    pub const SYS_EVENTFD2: usize = 19;
    // M5 Phase 1: File metadata & extended operations
    pub const SYS_STATFS: usize = 43;
    pub const SYS_FSTATFS: usize = 44;
    pub const SYS_FALLOCATE: usize = 47;
    pub const SYS_PREADV: usize = 69;
    pub const SYS_PWRITEV: usize = 70;
    pub const SYS_UTIMENSAT: usize = 88;
    pub const SYS_FADVISE64: usize = 223;
    pub const SYS_STATX: usize = 291;
    // M5 Phase 2: inotify
    pub const SYS_INOTIFY_INIT1: usize = 26;
    pub const SYS_INOTIFY_ADD_WATCH: usize = 27;
    pub const SYS_INOTIFY_RM_WATCH: usize = 28;
    // M5 Phase 3: Zero-copy I/O
    pub const SYS_SENDFILE: usize = 71;
    pub const SYS_SPLICE: usize = 76;
    pub const SYS_TEE: usize = 77;
    pub const SYS_COPY_FILE_RANGE: usize = 285;
    // M7 Phase 6: glibc syscall stubs
    pub const SYS_SCHED_SETSCHEDULER: usize = 119;
    pub const SYS_SCHED_GETSCHEDULER: usize = 121;
    pub const SYS_SCHED_SETAFFINITY: usize = 122;
    pub const SYS_RSEQ: usize = 293;
    pub const SYS_CLONE3: usize = 435;
    // M8 Phase 2: Namespaces
    pub const SYS_SETHOSTNAME: usize = 161;
    pub const SYS_SETDOMAINNAME: usize = 162;
    pub const SYS_UNSHARE: usize = 97;
    // M8 Phase 3: Filesystem isolation
    pub const SYS_PIVOT_ROOT: usize = 41;
    // M9 Phase 1: Syscall gap closure
    pub const SYS_FLOCK: usize = 32;
    pub const SYS_WAITID: usize = 95;
    pub const SYS_MEMFD_CREATE: usize = 279;
    pub const SYS_MKNOD: usize = 33;
    pub const SYS_SETTIMEOFDAY: usize = 170;
    pub const SYS_CLOCK_SETTIME: usize = 112;
    pub const SYS_NAME_TO_HANDLE_AT: usize = 264;
    pub const SYS_PIDFD_OPEN: usize = 434;
    pub const SYS_CLOSE_RANGE: usize = 436;
}

use syscall_numbers::*;

// ── PID 1 syscall trace ring buffer (for debugging systemd boot) ────

use core::sync::atomic::{AtomicUsize, Ordering as AtomOrd};

const TRACE_LEN: usize = 512;

struct TraceEntry {
    nr: usize,
    result: isize,
    args: [usize; 3], // first 3 args only
}

static TRACE_BUF: SpinLock<[TraceEntry; TRACE_LEN]> = SpinLock::new(
    [const { TraceEntry { nr: 0, result: 0, args: [0; 3] } }; TRACE_LEN]
);
static TRACE_IDX: AtomicUsize = AtomicUsize::new(0);

fn trace_pid1_syscall(nr: usize, a1: usize, a2: usize, a3: usize, result: isize) {
    let idx = TRACE_IDX.fetch_add(1, AtomOrd::Relaxed) % TRACE_LEN;
    let mut buf = TRACE_BUF.lock();
    buf[idx] = TraceEntry { nr, result, args: [a1, a2, a3] };
}

pub fn dump_pid1_trace() {
    let buf = TRACE_BUF.lock();
    let end = TRACE_IDX.load(AtomOrd::Relaxed);
    let start = if end > TRACE_LEN { end - TRACE_LEN } else { 0 };
    warn!("=== PID 1 last {} syscalls ===", end.min(TRACE_LEN));
    for i in start..end {
        let e = &buf[i % TRACE_LEN];
        let name = syscall_name_by_number(e.nr);
        warn!("  [{:3}] {}(n={}) args=({:#x},{:#x},{:#x}) -> {}",
              i, name, e.nr, e.args[0], e.args[1], e.args[2], e.result);
    }
}

use kevlar_platform::spinlock::SpinLock;

/// Stack-allocated path buffer for syscall path arguments.
///
/// Avoids 3 heap allocations per path-taking syscall (512-byte Vec for
/// UserCStr, String for owned copy, String for PathBuf).
struct StackPathBuf {
    buf: [u8; 256],
    len: usize,
}

impl StackPathBuf {
    fn from_user(uaddr: usize) -> Result<StackPathBuf> {
        let mut spb = StackPathBuf {
            buf: [0u8; 256],
            len: 0,
        };
        let va = UserVAddr::new_nonnull(uaddr)?;
        spb.len = va.read_cstr(&mut spb.buf)?;
        // Validate UTF-8.
        core::str::from_utf8(&spb.buf[..spb.len])
            .map_err(|_| Error::new(Errno::EINVAL))?;
        Ok(spb)
    }

    fn as_path(&self) -> &Path {
        // Safety: we validated UTF-8 in from_user.
        let s = core::str::from_utf8(&self.buf[..self.len]).unwrap();
        Path::new(s)
    }
}

fn resolve_path(uaddr: usize) -> Result<PathBuf> {
    const PATH_MAX: usize = 512;
    Ok(Path::new(UserCStr::new(UserVAddr::new_nonnull(uaddr)?, PATH_MAX)?.as_str()).to_path_buf())
}

pub struct SyscallHandler<'a> {
    pub frame: &'a mut PtRegs,
}

impl<'a> SyscallHandler<'a> {
    pub fn new(frame: &'a mut PtRegs) -> SyscallHandler<'a> {
        SyscallHandler { frame }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn dispatch(
        &mut self,
        a1: usize,
        a2: usize,
        a3: usize,
        a4: usize,
        a5: usize,
        a6: usize,
        n: usize,
    ) -> Result<isize> {
        // Single atomic load for all debug checks — avoids 5 separate
        // atomic loads on the fast path when debugging is disabled.
        let dbg = debug::get_filter();
        let dbg_syscall = dbg.contains(DebugFilter::SYSCALL);
        let dbg_canary = dbg.contains(DebugFilter::CANARY);

        // Syscall name is only needed for debug output — skip the 100-arm
        // match on the fast path.
        let pid;
        let name;
        if dbg_syscall || dbg_canary {
            let current = current_process();
            pid = current.pid().as_i32();
            name = syscall_name_by_number(n);

            if dbg_syscall {
                let is_stdio = (n == SYS_READ && a1 == 0)
                    || ((n == SYS_WRITE || n == SYS_WRITEV) && (a1 == 1 || a1 == 2));
                if !is_stdio {
                    debug::emit(DebugFilter::SYSCALL, &DebugEvent::SyscallEntry {
                        pid,
                        name,
                        number: n,
                        args: [a1, a2, a3, a4, a5, a6],
                    });
                }
            }
        } else {
            pid = 0;
            name = "";
        }

        // Stack canary check (pre-syscall, x86-64 only — reads FS.base TLS pointer).
        #[cfg(target_arch = "x86_64")]
        let pre_canary = if dbg_canary {
            let fsbase = current_process().arch().fsbase.load() as usize;
            debug::canary::check_and_emit(pid, fsbase, None, "pre_syscall", name)
        } else {
            None
        };
        #[cfg(not(target_arch = "x86_64"))]
        let pre_canary: Option<u64> = None;

        // Approximate kernel time: one tick per syscall.
        current_process().tick_stime();

        // Per-syscall cycle profiler: record TSC at entry.
        let prof_start = debug::profiler::syscall_enter();


        let ret = self.do_dispatch(a1, a2, a3, a4, a5, a6, n).map_err(|err| {
            if dbg_syscall {
                debug::emit(DebugFilter::SYSCALL, &DebugEvent::SyscallExit {
                    pid,
                    name,
                    number: n,
                    result: -(err.errno() as isize),
                    errno: Some(err.errno_name()),
                });
            }
            err
        });

        // Per-syscall cycle profiler: record TSC at exit.
        debug::profiler::syscall_exit(n, prof_start);

        // Record PID 1 syscalls for debugging systemd boot.
        if current_process().pid().as_i32() == 1 {
            let result = match &ret {
                Ok(v) => *v,
                Err(e) => -(e.errno() as isize),
            };
            trace_pid1_syscall(n, a1, a2, a3, result);


            // Decode path for openat/stat/access to make trace readable.
            // EXCLUDE execve — page table was switched, old user pointers are invalid!
            let path_arg = match n {
                SYS_OPENAT | SYS_NEWFSTATAT => Some(a2),
                SYS_STAT | SYS_LSTAT | SYS_ACCESS => Some(a1),
                SYS_READLINK | SYS_READLINKAT => Some(if n == SYS_READLINKAT { a2 } else { a1 }),
                _ => None,
            };
            if let Some(path_ptr) = path_arg {
                if path_ptr != 0 {
                    if let Ok(va) = UserVAddr::new_nonnull(path_ptr) {
                        let mut pbuf = [0u8; 128];
                        if va.read_cstr(&mut pbuf).is_ok() {
                            if let Ok(s) = core::str::from_utf8(&pbuf[..pbuf.iter().position(|&b| b == 0).unwrap_or(0)]) {
                                if !s.is_empty() {
                                    let name = syscall_name_by_number(n);
                                    warn!("  pid1 {}({}) -> {}", name, s, result);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Stack canary check (post-syscall, x86-64 only).
        #[cfg(target_arch = "x86_64")]
        if dbg_canary {
            let fsbase = current_process().arch().fsbase.load() as usize;
            debug::canary::check_and_emit(pid, fsbase, pre_canary, "post_syscall", name);
        }

        // Emit success result.
        if dbg_syscall {
            if let Ok(result) = &ret {
                let is_stdio = (n == SYS_READ && a1 == 0)
                    || ((n == SYS_WRITE || n == SYS_WRITEV) && (a1 == 1 || a1 == 2));
                if !is_stdio {
                    debug::emit(DebugFilter::SYSCALL, &DebugEvent::SyscallExit {
                        pid,
                        name,
                        number: n,
                        result: *result,
                        errno: None,
                    });
                }
            }
        }

        if let Err(err) = Process::try_delivering_signal(self.frame) {
            debug_warn!("failed to setup the signal stack: {:?}", err);
        }

        ret
    }

    #[allow(clippy::too_many_arguments)]
    pub fn do_dispatch(
        &mut self,
        a1: usize,
        a2: usize,
        a3: usize,
        a4: usize,
        a5: usize,
        a6: usize,
        n: usize,
    ) -> Result<isize> {
        match n {
            SYS_OPEN => {
                let p = StackPathBuf::from_user(a1)?;
                self.sys_open(
                    p.as_path(),
                    bitflags_from_user!(OpenFlags, a2 as i32)?,
                    FileMode::new(a3 as u32),
                )
            }
            SYS_CLOSE => self.sys_close(Fd::new(a1 as i32)),
            SYS_READ => self.sys_read(Fd::new(a1 as i32), UserVAddr::new_nonnull(a2)?, a3),
            SYS_WRITE => self.sys_write(Fd::new(a1 as i32), UserVAddr::new_nonnull(a2)?, a3),
            SYS_WRITEV => self.sys_writev(Fd::new(a1 as i32), UserVAddr::new_nonnull(a2)?, a3),
            SYS_MMAP => self.sys_mmap(
                UserVAddr::new(a1),
                a2 as c_size,
                bitflags_from_user!(MMapProt, a3 as c_int)?,
                // Unknown mmap hint flags (MAP_HUGETLB, MAP_LOCKED, etc.) are
                // silently ignored — they are hints, not semantic requirements.
                MMapFlags::from_bits_truncate(a4 as c_int),
                Fd::new(a5 as i32),
                a6 as c_off,
            ),
            SYS_STAT => {
                let p = StackPathBuf::from_user(a1)?;
                self.sys_stat(p.as_path(), UserVAddr::new_nonnull(a2)?)
            }
            SYS_FSTAT => self.sys_fstat(Fd::new(a1 as c_int), UserVAddr::new_nonnull(a2)?),
            SYS_LSTAT => {
                let p = StackPathBuf::from_user(a1)?;
                self.sys_lstat(p.as_path(), UserVAddr::new_nonnull(a2)?)
            }
            SYS_FCNTL => self.sys_fcntl(Fd::new(a1 as i32), a2 as c_int, a3),
            SYS_LINK => self.sys_link(&resolve_path(a1)?, &resolve_path(a2)?),
            SYS_LINKAT => self.sys_linkat(
                CwdOrFd::parse(a1 as c_int),
                &resolve_path(a2)?,
                CwdOrFd::parse(a3 as c_int),
                &resolve_path(a4)?,
                bitflags_from_user!(AtFlags, a5 as c_int)?,
            ),
            SYS_READLINK => self.sys_readlink(&resolve_path(a1)?, UserVAddr::new_nonnull(a2)?, a3),
            SYS_CHMOD => self.sys_chmod(&resolve_path(a1)?, FileMode::new(a2 as u32)),
            SYS_CHOWN => Ok(0), // TODO:
            SYS_FSYNC => self.sys_fsync(Fd::new(a1 as i32)),
            SYS_UTIMES => self.sys_utimes(&resolve_path(a1)?, UserVAddr::new(a2)),
            SYS_GETDENTS64 => {
                self.sys_getdents64(Fd::new(a1 as i32), UserVAddr::new_nonnull(a2)?, a3)
            }
            SYS_POLL => self.sys_poll(UserVAddr::new_nonnull(a1)?, a2 as c_ulong, a3 as c_int),
            SYS_SELECT => self.sys_select(
                a1 as c_int,
                UserVAddr::new(a2),
                UserVAddr::new(a3),
                UserVAddr::new(a4),
                UserVAddr::new(a5)
                    .map(|uaddr| uaddr.read::<Timeval>())
                    .transpose()?,
            ),
            SYS_DUP2 => self.sys_dup2(Fd::new(a1 as c_int), Fd::new(a2 as c_int)),
            SYS_GETCWD => self.sys_getcwd(UserVAddr::new_nonnull(a1)?, a2 as c_size),
            SYS_CHDIR => self.sys_chdir(&resolve_path(a1)?),
            SYS_MKDIR => self.sys_mkdir(&resolve_path(a1)?, FileMode::new(a2 as u32)),
            SYS_ARCH_PRCTL => self.sys_arch_prctl(a1 as i32, UserVAddr::new_nonnull(a2)?),
            SYS_BRK => self.sys_brk(UserVAddr::new(a1)),
            SYS_IOCTL => self.sys_ioctl(Fd::new(a1 as i32), a2, a3),
            SYS_GETPID => self.sys_getpid(),
            SYS_GETPGID => self.sys_getpgid(PId::new(a1 as i32)),
            SYS_GETUID => Ok(current_process().uid() as isize),
            SYS_GETEUID => Ok(current_process().euid() as isize),
            SYS_GETPRIORITY => self.sys_getpriority(a1 as i32, a2 as i32),
            SYS_SETPRIORITY => self.sys_setpriority(a1 as i32, a2 as i32, a3 as i32),
            SYS_SETUID => {
                current_process().set_uid(a1 as u32);
                current_process().set_euid(a1 as u32);
                Ok(0)
            }
            SYS_SETGID => {
                current_process().set_gid(a1 as u32);
                current_process().set_egid(a1 as u32);
                Ok(0)
            }
            SYS_SETGROUPS => Ok(0), // TODO:
            SYS_SETPGID => self.sys_setpgid(PId::new(a1 as i32), PgId::new(a2 as i32)),
            SYS_GETPPID => self.sys_getppid(),
            SYS_SET_TID_ADDRESS => self.sys_set_tid_address(UserVAddr::new_nonnull(a1)?),
            SYS_PIPE => self.sys_pipe(UserVAddr::new_nonnull(a1)?),
            SYS_RT_SIGACTION => self.sys_rt_sigaction(a1 as c_int, a2, UserVAddr::new(a3)),
            SYS_RT_SIGRETURN => self.sys_rt_sigreturn(),
            SYS_EXECVE => self.sys_execve(
                &resolve_path(a1)?,
                UserVAddr::new_nonnull(a2)?,
                UserVAddr::new_nonnull(a3)?,
            ),
            SYS_FORK => self.sys_fork(),
            SYS_CLONE => self.sys_clone(a1, a2, a3, a4, a5),
            SYS_WAIT4 => self.sys_wait4(
                PId::new(a1 as i32),
                UserVAddr::new(a2),
                bitflags_from_user!(WaitOptions, a3 as c_int)?,
                UserVAddr::new(a4),
            ),
            SYS_KILL => self.sys_kill(PId::new(a1 as i32), a2 as c_int),
            SYS_EXIT => self.sys_exit(a1 as i32),
            SYS_EXIT_GROUP => self.sys_exit_group(a1 as i32),
            SYS_SOCKET => self.sys_socket(a1 as i32, a2 as i32, a3 as i32),
            SYS_SOCKETPAIR => self.sys_socketpair(
                a1 as i32, a2 as i32, a3 as i32, UserVAddr::new_nonnull(a4)?,
            ),
            SYS_BIND => self.sys_bind(Fd::new(a1 as i32), UserVAddr::new_nonnull(a2)?, a3),
            SYS_SHUTDOWN => self.sys_shutdown(Fd::new(a1 as i32), a2 as i32),
            SYS_CONNECT => self.sys_connect(Fd::new(a1 as i32), UserVAddr::new_nonnull(a2)?, a3),
            SYS_LISTEN => self.sys_listen(Fd::new(a1 as i32), a2 as c_int),
            SYS_GETSOCKNAME => self.sys_getsockname(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                UserVAddr::new_nonnull(a3)?,
            ),
            SYS_GETPEERNAME => self.sys_getpeername(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                UserVAddr::new_nonnull(a3)?,
            ),
            SYS_GETSOCKOPT => self.sys_getsockopt(
                Fd::new(a1 as i32),
                a2 as c_int,
                a3 as c_int,
                UserVAddr::new(a4),
                UserVAddr::new(a5),
            ),
            SYS_ACCEPT => {
                self.sys_accept(Fd::new(a1 as i32), UserVAddr::new(a2), UserVAddr::new(a3))
            }
            SYS_SENDTO => self.sys_sendto(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3,
                bitflags_from_user!(SendToFlags, a4 as i32)?,
                UserVAddr::new(a5),
                a6,
            ),
            SYS_RECVFROM => self.sys_recvfrom(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3,
                bitflags_from_user!(RecvFromFlags, a4 as i32)?,
                UserVAddr::new(a5),
                UserVAddr::new(a6),
            ),
            SYS_UNAME => self.sys_uname(UserVAddr::new_nonnull(a1)?),
            SYS_CLOCK_GETTIME => {
                self.sys_clock_gettime(a1 as c_clockid, UserVAddr::new_nonnull(a2)?)
            }
            SYS_GETRANDOM => self.sys_getrandom(
                UserVAddr::new_nonnull(a1)?,
                a2,
                bitflags_from_user!(GetRandomFlags, a3 as c_uint)?,
            ),
            SYS_SYSLOG => self.sys_syslog(a1 as c_int, UserVAddr::new(a2), a3 as c_int),
            SYS_REBOOT => self.sys_reboot(a1 as c_int, a2 as c_int, a3),
            SYS_GETTID => self.sys_gettid(),
            SYS_RT_SIGPROCMASK => {
                self.sys_rt_sigprocmask(a1, UserVAddr::new(a2), UserVAddr::new(a3), a4)
            }
            // M1 Phase 1: Trivial syscalls
            SYS_SCHED_YIELD => self.sys_sched_yield(),
            SYS_SCHED_GETAFFINITY => self.sys_sched_getaffinity(
                a1 as c_int,
                a2,
                UserVAddr::new_nonnull(a3)?,
            ),
            SYS_DUP => self.sys_dup(Fd::new(a1 as c_int)),
            SYS_VFORK => self.sys_vfork(),
            SYS_UMASK => self.sys_umask(a1 as u32),
            SYS_GETGID => Ok(current_process().gid() as isize),
            SYS_GETEGID => Ok(current_process().egid() as isize),
            SYS_GETPGRP => self.sys_getpgrp(),
            // M1 Phase 2: FD plumbing
            SYS_DUP3 => self.sys_dup3(Fd::new(a1 as c_int), Fd::new(a2 as c_int), a3 as i32),
            SYS_PIPE2 => self.sys_pipe2(UserVAddr::new_nonnull(a1)?, a2 as c_int),
            // M1 Phase 3: *at syscalls + file ops
            SYS_LSEEK => self.sys_lseek(Fd::new(a1 as c_int), a2 as i64, a3 as c_int),
            SYS_ACCESS => {
                let p = StackPathBuf::from_user(a1)?;
                self.sys_access(p.as_path())
            }
            SYS_OPENAT => {
                let p = StackPathBuf::from_user(a2)?;
                self.sys_openat(
                    CwdOrFd::parse(a1 as c_int),
                    p.as_path(),
                    bitflags_from_user!(OpenFlags, a3 as i32)?,
                    FileMode::new(a4 as u32),
                )
            }
            SYS_NEWFSTATAT => {
                let p = StackPathBuf::from_user(a2)?;
                self.sys_newfstatat(
                    CwdOrFd::parse(a1 as c_int),
                    p.as_path(),
                    UserVAddr::new_nonnull(a3)?,
                    a4 as c_int,
                )
            }
            // M1 Phase 6: Memory management
            SYS_MPROTECT => self.sys_mprotect(
                UserVAddr::new_nonnull(a1)?,
                a2,
                bitflags_from_user!(MMapProt, a3 as c_int)?,
            ),
            SYS_MUNMAP => self.sys_munmap(UserVAddr::new_nonnull(a1)?, a2),
            // M1 Phase 4: Filesystem mutations
            SYS_UNLINK => self.sys_unlink(&resolve_path(a1)?),
            SYS_RMDIR => self.sys_rmdir(&resolve_path(a1)?),
            SYS_RENAME => self.sys_rename(&resolve_path(a1)?, &resolve_path(a2)?),
            // M1 Phase 5: Time & system info
            SYS_NANOSLEEP => self.sys_nanosleep(UserVAddr::new_nonnull(a1)?),
            SYS_GETTIMEOFDAY => self.sys_gettimeofday(UserVAddr::new_nonnull(a1)?),
            SYS_GETRLIMIT => self.sys_getrlimit(a1 as c_int, UserVAddr::new_nonnull(a2)?),
            SYS_SYSINFO => self.sys_sysinfo(UserVAddr::new_nonnull(a1)?),
            // ARM64-specific *at syscalls (also available on x86_64)
            SYS_FACCESSAT => {
                let p = StackPathBuf::from_user(a2)?;
                self.sys_access(p.as_path())
            }
            SYS_PPOLL => self.sys_poll(UserVAddr::new_nonnull(a1)?, a2 as c_ulong, -1 as c_int),
            SYS_PRLIMIT64 => {
                // prlimit64(pid, resource, new_rlim, old_rlim)
                // a3 = new_rlim (set), a4 = old_rlim (get) — either can be NULL.
                if a4 != 0 {
                    self.sys_getrlimit(a2 as c_int, UserVAddr::new_nonnull(a4)?)?;
                }
                // new_rlim (a3): accept and ignore — we don't enforce rlimits yet.
                Ok(0)
            }
            // M2: Dynamic linking
            SYS_PREAD64 => self.sys_pread64(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3,
                a4,
            ),
            SYS_MADVISE => self.sys_madvise(a1, a2, a3 as i32),
            SYS_FUTEX => self.sys_futex(a1, a2 as i32, a3 as u32, a4, a5, a6 as u32),
            SYS_SET_ROBUST_LIST => self.sys_set_robust_list(a1, a2),
            // M3: Terminal control, session management, *at syscalls, file ops
            SYS_SETSID => self.sys_setsid(),
            SYS_FCHDIR => self.sys_fchdir(Fd::new(a1 as c_int)),
            SYS_FTRUNCATE => self.sys_ftruncate(Fd::new(a1 as c_int), a2),
            SYS_GETRUSAGE => self.sys_getrusage(a1 as c_int, UserVAddr::new_nonnull(a2)?),
            SYS_READV => self.sys_readv(Fd::new(a1 as c_int), UserVAddr::new_nonnull(a2)?, a3),
            SYS_PWRITE64 => self.sys_pwrite64(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3,
                a4,
            ),
            SYS_READLINKAT => self.sys_readlinkat(
                CwdOrFd::parse(a1 as c_int),
                &resolve_path(a2)?,
                UserVAddr::new_nonnull(a3)?,
                a4,
            ),
            SYS_UNLINKAT => self.sys_unlinkat(
                CwdOrFd::parse(a1 as c_int),
                &resolve_path(a2)?,
                a3 as i32,
            ),
            SYS_MKDIRAT => self.sys_mkdirat(
                CwdOrFd::parse(a1 as c_int),
                &resolve_path(a2)?,
                FileMode::new(a3 as u32),
            ),
            SYS_GETSID => self.sys_getsid(PId::new(a1 as i32)),
            SYS_SIGALTSTACK => self.sys_sigaltstack(a1, a2),
            SYS_SYMLINK => self.sys_symlink(&resolve_path(a1)?, &resolve_path(a2)?),
            SYS_SYMLINKAT => self.sys_symlinkat(
                &resolve_path(a1)?,
                CwdOrFd::parse(a2 as c_int),
                &resolve_path(a3)?,
            ),
            SYS_RENAMEAT | SYS_RENAMEAT2 => self.sys_renameat(
                CwdOrFd::parse(a1 as c_int),
                &resolve_path(a2)?,
                CwdOrFd::parse(a3 as c_int),
                &resolve_path(a4)?,
            ),
            // M3 Phase 5: Job control + additional stubs
            SYS_TKILL => self.sys_tkill(a1 as c_int, a2 as c_int),
            SYS_TGKILL => self.sys_tgkill(a1 as c_int, a2 as c_int, a3 as c_int),
            SYS_RT_SIGSUSPEND => self.sys_rt_sigsuspend(UserVAddr::new_nonnull(a1)?, a2),
            SYS_FCHMOD => self.sys_fchmod(a1 as i32, a2 as u32),
            SYS_FCHOWN => Ok(0), // stub
            SYS_FCHMODAT => self.sys_fchmodat(
                a1 as i32,
                &resolve_path(a2)?,
                a3 as u32,
                a4 as i32,
            ),
            SYS_FCHOWNAT => self.sys_fchownat(
                a1 as i32,
                &resolve_path(a2)?,
                a3 as u32,
                a4 as u32,
                a5 as i32,
            ),
            SYS_PAUSE => self.sys_pause(),
            SYS_SETITIMER => self.sys_setitimer(
                a1 as i32,
                UserVAddr::new(a2),
                UserVAddr::new(a3),
            ),
            SYS_ALARM => self.sys_alarm(a1 as u32),
            SYS_GETGROUPS => self.sys_getgroups(a1, a2),
            // M4: epoll
            SYS_EPOLL_CREATE1 => self.sys_epoll_create1(a1 as c_int),
            SYS_EPOLL_CTL => self.sys_epoll_ctl(
                Fd::new(a1 as i32),
                a2 as c_int,
                Fd::new(a3 as i32),
                UserVAddr::new_nonnull(a4)?,
            ),
            SYS_EPOLL_WAIT | SYS_EPOLL_PWAIT => self.sys_epoll_wait(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3 as c_int,
                a4 as c_int,
            ),
            SYS_EVENTFD2 => self.sys_eventfd2(a1 as u32, a2 as c_int),
            SYS_TIMERFD_CREATE => self.sys_timerfd_create(a1 as c_int, a2 as c_int),
            SYS_TIMERFD_SETTIME => self.sys_timerfd_settime(
                Fd::new(a1 as i32),
                a2 as c_int,
                UserVAddr::new_nonnull(a3)?,
            ),
            SYS_SIGNALFD4 => self.sys_signalfd4(
                a1 as c_int,
                UserVAddr::new_nonnull(a2)?,
                a3,
                a4 as c_int,
            ),
            // M4 Phase 3: Unix sockets + sendmsg/recvmsg
            SYS_SENDMSG => self.sys_sendmsg(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3 as c_int,
            ),
            SYS_RECVMSG => self.sys_recvmsg(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3 as c_int,
            ),
            SYS_SETSOCKOPT => self.sys_setsockopt(
                Fd::new(a1 as i32),
                a2 as c_int,
                a3 as c_int,
                a4,
                a5,
            ),
            SYS_ACCEPT4 => self.sys_accept4(
                Fd::new(a1 as i32),
                UserVAddr::new(a2),
                UserVAddr::new(a3),
                a4 as c_int,
            ),
            // M4 Phase 4: Filesystem mounting
            SYS_MOUNT => self.sys_mount(
                UserVAddr::new(a1),
                UserVAddr::new_nonnull(a2)?,
                UserVAddr::new(a3),
                a4 as c_int,
                a5,
            ),
            SYS_UMOUNT2 => self.sys_umount2(
                UserVAddr::new_nonnull(a1)?,
                a2 as c_int,
            ),
            // M4 Phase 5: Process management & capabilities
            SYS_PRCTL => self.sys_prctl(a1 as c_int, a2, a3, a4, a5),
            SYS_CAPGET => self.sys_capget(UserVAddr::new_nonnull(a1)?, a2),
            SYS_CAPSET => self.sys_capset(UserVAddr::new_nonnull(a1)?, a2),
            // M5 Phase 1: File metadata & extended operations
            SYS_STATFS => {
                let p = StackPathBuf::from_user(a1)?;
                self.sys_statfs(p.as_path(), UserVAddr::new_nonnull(a2)?)
            }
            SYS_FSTATFS => self.sys_fstatfs(Fd::new(a1 as i32), UserVAddr::new_nonnull(a2)?),
            SYS_UTIMENSAT => self.sys_utimensat(
                CwdOrFd::parse(a1 as c_int),
                a2,
                UserVAddr::new(a3),
                a4 as i32,
            ),
            SYS_STATX => self.sys_statx(
                CwdOrFd::parse(a1 as c_int),
                a2,
                a3 as i32,
                a4 as u32,
                UserVAddr::new_nonnull(a5)?,
            ),
            SYS_FALLOCATE => self.sys_fallocate(
                Fd::new(a1 as i32),
                a2 as i32,
                a3 as i64,
                a4 as i64,
            ),
            SYS_FADVISE64 => self.sys_fadvise64(
                Fd::new(a1 as i32),
                a2 as i64,
                a3 as i64,
                a4 as i32,
            ),
            SYS_PREADV => self.sys_preadv(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3,
                a4,
            ),
            SYS_PWRITEV => self.sys_pwritev(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3,
                a4,
            ),
            // M5 Phase 2: inotify
            SYS_INOTIFY_INIT1 => self.sys_inotify_init1(a1 as c_int),
            SYS_INOTIFY_ADD_WATCH => self.sys_inotify_add_watch(
                Fd::new(a1 as i32),
                UserVAddr::new_nonnull(a2)?,
                a3 as u32,
            ),
            SYS_INOTIFY_RM_WATCH => self.sys_inotify_rm_watch(
                Fd::new(a1 as i32),
                a2 as c_int,
            ),
            // M5 Phase 3: Zero-copy I/O
            SYS_SENDFILE => self.sys_sendfile(
                Fd::new(a1 as i32),
                Fd::new(a2 as i32),
                UserVAddr::new(a3),
                a4,
            ),
            SYS_SPLICE => self.sys_splice(
                Fd::new(a1 as i32),
                UserVAddr::new(a2),
                Fd::new(a3 as i32),
                UserVAddr::new(a4),
                a5,
                a6 as u32,
            ),
            SYS_TEE => self.sys_tee(
                Fd::new(a1 as i32),
                Fd::new(a2 as i32),
                a3,
                a4 as u32,
            ),
            SYS_COPY_FILE_RANGE => self.sys_copy_file_range(
                Fd::new(a1 as i32),
                UserVAddr::new(a2),
                Fd::new(a3 as i32),
                UserVAddr::new(a4),
                a5,
                a6 as u32,
            ),
            // M7 Phase 6: glibc syscall stubs
            SYS_RSEQ => self.sys_rseq(a1, a2 as u32, a3 as i32, a4 as u32),
            SYS_CLONE3 => self.sys_clone3(a1, a2),
            SYS_SCHED_SETAFFINITY => self.sys_sched_setaffinity(a1 as i32, a2, a3),
            SYS_SCHED_GETSCHEDULER => self.sys_sched_getscheduler(a1 as i32),
            SYS_SCHED_SETSCHEDULER => self.sys_sched_setscheduler(a1 as i32, a2 as i32, a3),
            // M8 Phase 2: Namespaces
            SYS_SETHOSTNAME => self.sys_sethostname(UserVAddr::new_nonnull(a1)?, a2),
            SYS_SETDOMAINNAME => self.sys_setdomainname(UserVAddr::new_nonnull(a1)?, a2),
            SYS_UNSHARE => self.sys_unshare(a1),
            // M8 Phase 3: Filesystem isolation
            SYS_PIVOT_ROOT => self.sys_pivot_root(a1, a2),
            // M9 Phase 1: Syscall gap closure
            SYS_FLOCK => self.sys_flock(a1 as i32, a2 as i32),
            SYS_WAITID => self.sys_waitid(
                a1 as c_int,
                a2 as c_int,
                UserVAddr::new_nonnull(a3)?,
                a4 as c_int,
            ),
            SYS_MEMFD_CREATE => self.sys_memfd_create(UserVAddr::new_nonnull(a1)?, a2 as u32),
            // mknod: systemd creates device nodes. Stub — our devfs already has them.
            SYS_MKNOD => Ok(0),
            // settimeofday: ignore time adjustments.
            SYS_SETTIMEOFDAY => Ok(0),
            // clock_settime: ignore clock adjustments.
            SYS_CLOCK_SETTIME => Ok(0),
            SYS_NAME_TO_HANDLE_AT => self.sys_name_to_handle_at(
                CwdOrFd::parse(a1 as c_int),
                a2,
                UserVAddr::new_nonnull(a3)?,
                UserVAddr::new_nonnull(a4)?,
                a5 as i32,
            ),
            SYS_PIDFD_OPEN => self.sys_pidfd_open(a1 as i32, a2 as u32),
            SYS_CLOSE_RANGE => self.sys_close_range(a1 as u32, a2 as u32, a3 as u32),
            _ => {
                let pid = current_process().pid().as_i32();
                debug::emit(DebugFilter::SYSCALL, &DebugEvent::UnimplementedSyscall {
                    pid,
                    name: syscall_name_by_number(n),
                    number: n,
                });
                warn!(
                    "unimplemented system call: {} (n={}) pid={}",
                    syscall_name_by_number(n),
                    n,
                    current_process().pid().as_i32(),
                );
                Err(Error::new(Errno::ENOSYS))
            }
        }
    }
}

pub fn syscall_name_by_number(n: usize) -> &'static str {
    match n {
        SYS_READ => "read",
        SYS_WRITE => "write",
        SYS_OPEN => "open",
        SYS_CLOSE => "close",
        SYS_STAT => "stat",
        SYS_FSTAT => "fstat",
        SYS_LSTAT => "lstat",
        SYS_POLL => "poll",
        SYS_LSEEK => "lseek",
        SYS_MMAP => "mmap",
        SYS_MPROTECT => "mprotect",
        SYS_MUNMAP => "munmap",
        SYS_BRK => "brk",
        SYS_RT_SIGACTION => "rt_sigaction",
        SYS_RT_SIGPROCMASK => "rt_sigprocmask",
        SYS_RT_SIGRETURN => "rt_sigreturn",
        SYS_IOCTL => "ioctl",
        SYS_WRITEV => "writev",
        SYS_ACCESS => "access",
        SYS_PIPE => "pipe",
        SYS_SELECT => "select",
        SYS_SCHED_YIELD => "sched_yield",
        SYS_SCHED_GETAFFINITY => "sched_getaffinity",
        SYS_DUP => "dup",
        SYS_DUP2 => "dup2",
        SYS_NANOSLEEP => "nanosleep",
        SYS_GETPID => "getpid",
        SYS_SOCKET => "socket",
        SYS_SOCKETPAIR => "socketpair",
        SYS_CONNECT => "connect",
        SYS_ACCEPT => "accept",
        SYS_SENDTO => "sendto",
        SYS_RECVFROM => "recvfrom",
        SYS_SHUTDOWN => "shutdown",
        SYS_BIND => "bind",
        SYS_LISTEN => "listen",
        SYS_GETSOCKNAME => "getsockname",
        SYS_GETPEERNAME => "getpeername",
        SYS_GETSOCKOPT => "getsockopt",
        SYS_FORK => "fork",
        SYS_VFORK => "vfork",
        SYS_EXECVE => "execve",
        SYS_EXIT => "exit",
        SYS_WAIT4 => "wait4",
        SYS_KILL => "kill",
        SYS_UNAME => "uname",
        SYS_FCNTL => "fcntl",
        SYS_FSYNC => "fsync",
        SYS_GETCWD => "getcwd",
        SYS_CHDIR => "chdir",
        SYS_RENAME => "rename",
        SYS_MKDIR => "mkdir",
        SYS_RMDIR => "rmdir",
        SYS_LINK => "link",
        SYS_UNLINK => "unlink",
        SYS_READLINK => "readlink",
        SYS_CHMOD => "chmod",
        SYS_CHOWN => "chown",
        SYS_UMASK => "umask",
        SYS_GETTIMEOFDAY => "gettimeofday",
        SYS_GETRLIMIT => "getrlimit",
        SYS_SYSINFO => "sysinfo",
        SYS_GETUID => "getuid",
        SYS_SYSLOG => "syslog",
        SYS_GETGID => "getgid",
        SYS_SETUID => "setuid",
        SYS_SETGID => "setgid",
        SYS_GETEUID => "geteuid",
        SYS_GETEGID => "getegid",
        SYS_SETPGID => "setpgid",
        SYS_GETPPID => "getppid",
        SYS_GETPGRP => "getpgrp",
        SYS_SETGROUPS => "setgroups",
        SYS_GETPGID => "getpgid",
        SYS_ARCH_PRCTL => "arch_prctl",
        SYS_REBOOT => "reboot",
        SYS_GETTID => "gettid",
        SYS_GETDENTS64 => "getdents64",
        SYS_SET_TID_ADDRESS => "set_tid_address",
        SYS_CLOCK_GETTIME => "clock_gettime",
        SYS_EXIT_GROUP => "exit_group",
        SYS_UTIMES => "utimes",
        SYS_OPENAT => "openat",
        SYS_NEWFSTATAT => "newfstatat",
        SYS_LINKAT => "linkat",
        SYS_DUP3 => "dup3",
        SYS_PIPE2 => "pipe2",
        SYS_GETRANDOM => "getrandom",
        SYS_CLONE => "clone",
        SYS_FACCESSAT => "faccessat",
        SYS_PPOLL => "ppoll",
        SYS_PRLIMIT64 => "prlimit64",
        SYS_PREAD64 => "pread64",
        SYS_MADVISE => "madvise",
        SYS_FUTEX => "futex",
        SYS_SET_ROBUST_LIST => "set_robust_list",
        SYS_SETSID => "setsid",
        SYS_FCHDIR => "fchdir",
        SYS_FTRUNCATE => "ftruncate",
        SYS_GETRUSAGE => "getrusage",
        SYS_READV => "readv",
        SYS_PWRITE64 => "pwrite64",
        SYS_READLINKAT => "readlinkat",
        SYS_UNLINKAT => "unlinkat",
        SYS_MKDIRAT => "mkdirat",
        SYS_RENAMEAT => "renameat",
        SYS_RENAMEAT2 => "renameat2",
        SYS_SYMLINK => "symlink",
        SYS_SYMLINKAT => "symlinkat",
        SYS_GETSID => "getsid",
        SYS_SIGALTSTACK => "sigaltstack",
        SYS_TKILL => "tkill",
        SYS_TGKILL => "tgkill",
        SYS_RT_SIGSUSPEND => "rt_sigsuspend",
        SYS_FCHMOD => "fchmod",
        SYS_FCHOWN => "fchown",
        SYS_FCHMODAT => "fchmodat",
        SYS_FCHOWNAT => "fchownat",
        SYS_PAUSE => "pause",
        SYS_SETITIMER => "setitimer",
        SYS_ALARM => "alarm",
        SYS_GETGROUPS => "getgroups",
        SYS_EPOLL_CREATE1 => "epoll_create1",
        SYS_EPOLL_CTL => "epoll_ctl",
        SYS_EPOLL_WAIT => "epoll_wait",
        SYS_EPOLL_PWAIT => "epoll_pwait",
        SYS_EVENTFD2 => "eventfd2",
        SYS_TIMERFD_CREATE => "timerfd_create",
        SYS_TIMERFD_SETTIME => "timerfd_settime",
        SYS_SIGNALFD4 => "signalfd4",
        SYS_SENDMSG => "sendmsg",
        SYS_RECVMSG => "recvmsg",
        SYS_SETSOCKOPT => "setsockopt",
        SYS_ACCEPT4 => "accept4",
        SYS_MOUNT => "mount",
        SYS_UMOUNT2 => "umount2",
        SYS_PRCTL => "prctl",
        SYS_CAPGET => "capget",
        SYS_CAPSET => "capset",
        SYS_GETPRIORITY => "getpriority",
        SYS_SETPRIORITY => "setpriority",
        SYS_STATFS => "statfs",
        SYS_FSTATFS => "fstatfs",
        SYS_UTIMENSAT => "utimensat",
        SYS_STATX => "statx",
        SYS_FALLOCATE => "fallocate",
        SYS_FADVISE64 => "fadvise64",
        SYS_PREADV => "preadv",
        SYS_PWRITEV => "pwritev",
        SYS_INOTIFY_INIT1 => "inotify_init1",
        SYS_INOTIFY_ADD_WATCH => "inotify_add_watch",
        SYS_INOTIFY_RM_WATCH => "inotify_rm_watch",
        SYS_SENDFILE => "sendfile",
        SYS_SPLICE => "splice",
        SYS_TEE => "tee",
        SYS_COPY_FILE_RANGE => "copy_file_range",
        SYS_RSEQ => "rseq",
        SYS_CLONE3 => "clone3",
        SYS_SCHED_SETAFFINITY => "sched_setaffinity",
        SYS_SCHED_GETSCHEDULER => "sched_getscheduler",
        SYS_SCHED_SETSCHEDULER => "sched_setscheduler",
        SYS_SETHOSTNAME => "sethostname",
        SYS_SETDOMAINNAME => "setdomainname",
        SYS_UNSHARE => "unshare",
        SYS_PIVOT_ROOT => "pivot_root",
        SYS_FLOCK => "flock",
        SYS_WAITID => "waitid",
        SYS_MEMFD_CREATE => "memfd_create",
        SYS_MKNOD => "mknod",
        SYS_SETTIMEOFDAY => "settimeofday",
        SYS_CLOCK_SETTIME => "clock_settime",
        SYS_NAME_TO_HANDLE_AT => "name_to_handle_at",
        SYS_PIDFD_OPEN => "pidfd_open",
        SYS_CLOSE_RANGE => "close_range",
        _ => "(unknown)",
    }
}
