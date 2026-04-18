// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! /proc filesystem.
//!
//! Provides per-process directories (/proc/[pid]/, /proc/self) and
//! system-wide files (/proc/mounts, /proc/meminfo, /proc/stat, etc.).
use core::fmt;

use crate::{
    fs::{
        file_system::FileSystem,
        inode::{Directory, FileLike, PollStatus},
    },
    process::{list_pids, PId},
    result::Result,
};
use alloc::string::ToString;
use alloc::sync::Arc;
use kevlar_vfs::{
    inode::{DirEntry, FileType, INode, INodeNo},
    stat::{FileMode, GId, Stat, UId, S_IFDIR},
    user_buffer::UserBuffer,
};
use kevlar_utils::once::Once;

use self::metrics::MetricsFile;
use self::proc_self::{ProcPidDir, ProcSelfSymlink};
use self::system::*;

use super::tmpfs::TmpFs;

mod metrics;
pub mod proc_self;
pub mod system;

pub static PROC_FS: Once<Arc<ProcFs>> = Once::new();

pub struct ProcFs {
    /// Underlying tmpfs for static entries.
    #[allow(dead_code)]
    tmpfs: TmpFs,
    /// Dynamic root directory that intercepts PID lookups.
    root: Arc<ProcRootDir>,
}

impl ProcFs {
    pub fn new() -> ProcFs {
        let tmpfs = TmpFs::new();
        let static_root = tmpfs.root_tmpfs_dir();

        // Add static system-wide files.
        static_root.add_file("metrics", Arc::new(MetricsFile::new()) as Arc<dyn FileLike>);
        static_root.add_file("mounts", Arc::new(ProcMountsFile) as Arc<dyn FileLike>);
        static_root.add_file("filesystems", Arc::new(ProcFilesystemsFile) as Arc<dyn FileLike>);
        static_root.add_file("cmdline", Arc::new(ProcCmdlineFile) as Arc<dyn FileLike>);
        static_root.add_file("stat", Arc::new(ProcStatFile) as Arc<dyn FileLike>);
        static_root.add_file("meminfo", Arc::new(ProcMeminfoFile) as Arc<dyn FileLike>);
        static_root.add_file("version", Arc::new(ProcVersionFile) as Arc<dyn FileLike>);
        static_root.add_file("cpuinfo", Arc::new(ProcCpuinfoFile) as Arc<dyn FileLike>);
        static_root.add_file("uptime", Arc::new(ProcUptimeFile) as Arc<dyn FileLike>);
        static_root.add_file("loadavg", Arc::new(ProcLoadavgFile) as Arc<dyn FileLike>);

        // /proc/sys/ hierarchy for systemd compatibility.
        let sys_dir = static_root.add_dir("sys");
        let sys_kernel_dir = sys_dir.add_dir("kernel");
        sys_kernel_dir.add_file("hostname", Arc::new(ProcSysHostname) as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("osrelease", Arc::new(ProcSysStaticFile("6.19.8\n")) as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("ostype", Arc::new(ProcSysStaticFile("Linux\n")) as Arc<dyn FileLike>);
        let sys_random_dir = sys_kernel_dir.add_dir("random");
        sys_random_dir.add_file("boot_id", Arc::new(ProcSysBootId) as Arc<dyn FileLike>);
        sys_random_dir.add_file("uuid", Arc::new(ProcSysRandomUuid) as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("cap_last_cap", Arc::new(ProcSysStaticFile("40\n")) as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("pid_max", ProcSysMutableFile::new("32768\n") as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("threads-max", ProcSysMutableFile::new("32768\n") as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("overflowuid", Arc::new(ProcSysStaticFile("65534\n")) as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("overflowgid", Arc::new(ProcSysStaticFile("65534\n")) as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("kptr_restrict", ProcSysMutableFile::new("1\n") as Arc<dyn FileLike>);
        sys_kernel_dir.add_file("dmesg_restrict", ProcSysMutableFile::new("0\n") as Arc<dyn FileLike>);
        let sys_vm_dir = sys_dir.add_dir("vm");
        sys_vm_dir.add_file("overcommit_memory", ProcSysMutableFile::new("0\n") as Arc<dyn FileLike>);
        sys_vm_dir.add_file("max_map_count", ProcSysMutableFile::new("65530\n") as Arc<dyn FileLike>);
        let sys_fs_dir = sys_dir.add_dir("fs");
        sys_fs_dir.add_file("nr_open", Arc::new(ProcSysStaticFile("1048576\n")) as Arc<dyn FileLike>);
        sys_fs_dir.add_file("file-max", Arc::new(ProcSysStaticFile("1048576\n")) as Arc<dyn FileLike>);
        let sys_net_dir = sys_dir.add_dir("net");
        let sys_net_ipv4_dir = sys_net_dir.add_dir("ipv4");
        sys_net_ipv4_dir.add_file("ip_forward", ProcSysMutableFile::new("0\n") as Arc<dyn FileLike>);
        sys_net_ipv4_dir.add_file("tcp_syncookies", ProcSysMutableFile::new("1\n") as Arc<dyn FileLike>);
        sys_net_ipv4_dir.add_file("ip_local_port_range", ProcSysMutableFile::new("32768\t60999\n") as Arc<dyn FileLike>);
        let sys_net_core_dir = sys_net_dir.add_dir("core");
        sys_net_core_dir.add_file("somaxconn", ProcSysMutableFile::new("4096\n") as Arc<dyn FileLike>);
        sys_net_core_dir.add_file("rmem_default", ProcSysMutableFile::new("212992\n") as Arc<dyn FileLike>);
        sys_net_core_dir.add_file("wmem_default", ProcSysMutableFile::new("212992\n") as Arc<dyn FileLike>);
        let sys_net_unix_dir = sys_net_dir.add_dir("unix");
        sys_net_unix_dir.add_file("max_dgram_qlen", Arc::new(ProcSysStaticFile("512\n")) as Arc<dyn FileLike>);

        // /proc/net/ — network info stubs for tools like ifconfig.
        let net_dir = static_root.add_dir("net");
        net_dir.add_file("dev", Arc::new(ProcNetDevFile) as Arc<dyn FileLike>);
        net_dir.add_file("if_inet6", Arc::new(ProcSysStaticFile("")) as Arc<dyn FileLike>);
        net_dir.add_file("tcp", Arc::new(ProcNetTcpFile) as Arc<dyn FileLike>);
        net_dir.add_file("udp", Arc::new(ProcNetUdpFile) as Arc<dyn FileLike>);
        net_dir.add_file("tcp6", Arc::new(ProcSysStaticFile(
            "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n"
        )) as Arc<dyn FileLike>);
        net_dir.add_file("udp6", Arc::new(ProcSysStaticFile(
            "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n"
        )) as Arc<dyn FileLike>);

        // /proc/cgroups — list available cgroup controllers.
        static_root.add_file("cgroups", Arc::new(ProcSysStaticFile(
            "#subsys_name\thierarchy\tnum_cgroups\tenabled\n"
        )) as Arc<dyn FileLike>);

        let static_dir: Arc<dyn Directory> = tmpfs.root_dir().unwrap();

        let root = Arc::new(ProcRootDir {
            static_dir,
        });

        ProcFs { tmpfs, root }
    }
}

impl FileSystem for ProcFs {
    fn root_dir(&self) -> Result<Arc<dyn Directory>> {
        Ok(self.root.clone())
    }
}

pub fn init() {
    PROC_FS.init(|| Arc::new(ProcFs::new()));
}

// ── Dynamic /proc root directory ────────────────────────────────────

/// Root directory of /proc. Intercepts lookups for "self" and numeric
/// PID directories, delegates everything else to the static tmpfs.
struct ProcRootDir {
    static_dir: Arc<dyn Directory>,
}

impl fmt::Debug for ProcRootDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcRootDir").finish()
    }
}

impl Directory for ProcRootDir {
    fn lookup(&self, name: &str) -> Result<INode> {
        // "self" → symlink to /proc/<current_pid>
        if name == "self" {
            return Ok(INode::Symlink(
                Arc::new(ProcSelfSymlink) as Arc<dyn kevlar_vfs::inode::Symlink>
            ));
        }

        // Numeric name → per-PID directory.
        if let Ok(pid_val) = name.parse::<i32>() {
            if pid_val > 0 {
                return Ok(INode::Directory(
                    ProcPidDir::new(PId::new(pid_val)) as Arc<dyn Directory>
                ));
            }
        }

        // Fall through to static entries (metrics, mounts, etc.).
        self.static_dir.lookup(name)
    }

    fn create_file(&self, name: &str, mode: FileMode, uid: UId, gid: GId) -> Result<INode> {
        self.static_dir.create_file(name, mode, uid, gid)
    }

    fn create_dir(&self, name: &str, mode: FileMode, uid: UId, gid: GId) -> Result<INode> {
        self.static_dir.create_dir(name, mode, uid, gid)
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFDIR | 0o555),
            ..Stat::zeroed()
        })
    }

    fn inode_no(&self) -> Result<INodeNo> {
        self.static_dir.inode_no()
    }

    fn readdir(&self, index: usize) -> Result<Option<DirEntry>> {
        // Static entries first (metrics, mounts, etc.).
        if let Some(entry) = self.static_dir.readdir(index)? {
            return Ok(Some(entry));
        }

        // Count static entries to compute the dynamic offset.
        let mut static_count = 0;
        while self.static_dir.readdir(static_count)?.is_some() {
            static_count += 1;
        }

        let dynamic_index = index - static_count;

        // First dynamic entry: "self" symlink.
        if dynamic_index == 0 {
            return Ok(Some(DirEntry {
                inode_no: INodeNo::new(0),
                file_type: FileType::Link,
                name: "self".into(),
            }));
        }

        // Remaining dynamic entries: one per PID.
        let pids = list_pids();
        let pid_index = dynamic_index - 1;
        if pid_index < pids.len() {
            return Ok(Some(DirEntry {
                inode_no: INodeNo::new(pids[pid_index].as_i32() as usize),
                file_type: FileType::Directory,
                name: pids[pid_index].as_i32().to_string(),
            }));
        }

        Ok(None)
    }

    fn link(&self, name: &str, link_to: &INode) -> Result<()> {
        self.static_dir.link(name, link_to)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        self.static_dir.unlink(name)
    }
}

// ── /proc/sys/ helper file types ────────────────────────────────────

use kevlar_vfs::{
    inode::OpenOptions,
    stat::S_IFREG,
    user_buffer::{UserBufWriter, UserBufferMut},
};

/// Static /proc/sys/ file returning a fixed string.
struct ProcSysStaticFile(&'static str);

impl fmt::Debug for ProcSysStaticFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcSysStaticFile")
    }
}

impl FileLike for ProcSysStaticFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat { mode: FileMode::new(S_IFREG | 0o644), ..Stat::zeroed() })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 { return Ok(0); }
        let bytes = self.0.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &kevlar_vfs::inode::OpenOptions) -> Result<usize> {
        // Accept writes silently (systemd bumps sysctl values at boot).
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        // Proc files always have data available for reading.
        Ok(PollStatus::POLLIN)
    }
}

/// A /proc/sys file that remembers writes (read-after-write consistent).
/// Used for tunables like overcommit_memory, ip_forward, etc.
struct ProcSysMutableFile {
    value: kevlar_platform::spinlock::SpinLock<alloc::string::String>,
}

impl ProcSysMutableFile {
    fn new(initial: &str) -> Arc<Self> {
        Arc::new(ProcSysMutableFile {
            value: kevlar_platform::spinlock::SpinLock::new(alloc::string::String::from(initial)),
        })
    }
}

impl fmt::Debug for ProcSysMutableFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcSysMutableFile")
    }
}

impl FileLike for ProcSysMutableFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat { mode: FileMode::new(S_IFREG | 0o644), ..Stat::zeroed() })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 { return Ok(0); }
        let val = self.value.lock_no_irq();
        let bytes = val.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        use kevlar_vfs::user_buffer::UserBufReader;
        let mut data = [0u8; 256];
        let mut reader = UserBufReader::from(buf);
        let n = reader.read_bytes(&mut data)?;
        let s = core::str::from_utf8(&data[..n]).unwrap_or("0\n");
        *self.value.lock_no_irq() = alloc::string::String::from(s);
        Ok(n)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus::POLLIN)
    }
}

/// /proc/sys/kernel/hostname — returns UTS namespace hostname.
struct ProcSysHostname;

impl fmt::Debug for ProcSysHostname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcSysHostname")
    }
}

impl FileLike for ProcSysHostname {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat { mode: FileMode::new(S_IFREG | 0o644), ..Stat::zeroed() })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 { return Ok(0); }
        let ns = crate::process::current_process().namespaces();
        let hostname = ns.uts.get_hostname();
        let mut s = alloc::string::String::from_utf8_lossy(&hostname).into_owned();
        s.push('\n');
        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        use kevlar_vfs::user_buffer::UserBufReader;
        let mut data = [0u8; 64];
        let mut reader = UserBufReader::from(buf);
        let n = reader.read_bytes(&mut data)?;
        let len = if n > 0 && data[n - 1] == b'\n' { n - 1 } else { n };
        crate::process::current_process().namespaces().uts.set_hostname(&data[..len])?;
        Ok(n)
    }
}

/// /proc/sys/kernel/random/boot_id — random UUID, generated once per boot.
struct ProcSysBootId;

/// Cached boot UUID string (37 bytes: 36 hex+dash + newline).
static BOOT_ID: ::spin::Once<[u8; 37]> = ::spin::Once::new();

impl fmt::Debug for ProcSysBootId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcSysBootId")
    }
}

impl FileLike for ProcSysBootId {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat { mode: FileMode::new(S_IFREG | 0o444), ..Stat::zeroed() })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 { return Ok(0); }
        let id_bytes = BOOT_ID.call_once(|| {
            let mut bytes = [0u8; 16];
            kevlar_platform::random::rdrand_fill(&mut bytes);
            // Format as UUID v4: xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
            bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
            bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
            let s = alloc::format!(
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}\n",
                bytes[0], bytes[1], bytes[2], bytes[3],
                bytes[4], bytes[5], bytes[6], bytes[7],
                bytes[8], bytes[9], bytes[10], bytes[11],
                bytes[12], bytes[13], bytes[14], bytes[15],
            );
            let sb = s.as_bytes();
            let mut out = [0u8; 37];
            out[..sb.len()].copy_from_slice(sb);
            out
        });
        let len = core::cmp::min(id_bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&id_bytes[..len])?;
        Ok(len)
    }
}

/// /proc/sys/kernel/random/uuid — generates a fresh UUID v4 on each read.
/// Used by systemd and other programs that need unique identifiers.
struct ProcSysRandomUuid;

impl fmt::Debug for ProcSysRandomUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcSysRandomUuid")
    }
}

impl FileLike for ProcSysRandomUuid {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat { mode: FileMode::new(S_IFREG | 0o444), ..Stat::zeroed() })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 { return Ok(0); }
        let mut bytes = [0u8; 16];
        kevlar_platform::random::rdrand_fill(&mut bytes);
        bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
        let s = alloc::format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}\n",
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15],
        );
        let sb = s.as_bytes();
        let len = core::cmp::min(sb.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&sb[..len])?;
        Ok(len)
    }
}
