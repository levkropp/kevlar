// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! System-wide /proc files: /proc/mounts, /proc/filesystems, /proc/cmdline,
//! /proc/stat, /proc/meminfo, /proc/version, /proc/cpuinfo, /proc/uptime,
//! /proc/loadavg.
use core::fmt;

use kevlar_platform::page_allocator::read_allocator_stats;
use kevlar_vfs::{
    inode::{FileLike, OpenOptions},
    result::Result,
    stat::{FileMode, Stat, S_IFREG},
    user_buffer::{UserBufWriter, UserBufferMut},
};

use crate::fs::mount::MountTable;
use crate::process::{process_count, read_process_stats};
use crate::timer::read_monotonic_clock;

// ── /proc/mounts ────────────────────────────────────────────────────

pub struct ProcMountsFile;

impl fmt::Debug for ProcMountsFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcMountsFile").finish()
    }
}

impl FileLike for ProcMountsFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        let mut s = alloc::string::String::new();
        MountTable::format_mounts(&mut s);

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/filesystems ───────────────────────────────────────────────

pub struct ProcFilesystemsFile;

impl fmt::Debug for ProcFilesystemsFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcFilesystemsFile").finish()
    }
}

impl FileLike for ProcFilesystemsFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        let s = "nodev\tproc\nnodev\tsysfs\nnodev\ttmpfs\nnodev\tdevtmpfs\nnodev\tcgroup2\n";
        let len = core::cmp::min(s.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&s.as_bytes()[..len])?;
        Ok(len)
    }
}

// ── /proc/cmdline ───────────────────────────────────────────────────

pub struct ProcCmdlineFile;

impl fmt::Debug for ProcCmdlineFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcCmdlineFile").finish()
    }
}

impl FileLike for ProcCmdlineFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        // Provide a minimal kernel command line.
        let s = "kevlar\n";
        let len = core::cmp::min(s.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&s.as_bytes()[..len])?;
        Ok(len)
    }
}

// ── /proc/stat ──────────────────────────────────────────────────────

pub struct ProcStatFile;

impl fmt::Debug for ProcStatFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcStatFile").finish()
    }
}

impl FileLike for ProcStatFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        use core::fmt::Write;
        let mut s = alloc::string::String::new();

        let uptime_ms = read_monotonic_clock().msecs();
        let procs = process_count();
        let stats = read_process_stats();

        // Minimal /proc/stat with cpu line and process counts.
        let _ = write!(s, "cpu  0 0 0 {} 0 0 0 0 0 0\n", uptime_ms / 10);
        let _ = write!(s, "processes {}\n", stats.fork_total);
        let _ = write!(s, "procs_running {}\n", procs);
        let _ = write!(s, "procs_blocked 0\n");
        let _ = write!(s, "btime 0\n");

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/meminfo ───────────────────────────────────────────────────

pub struct ProcMeminfoFile;

impl fmt::Debug for ProcMeminfoFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcMeminfoFile").finish()
    }
}

impl FileLike for ProcMeminfoFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        use core::fmt::Write;
        let mut s = alloc::string::String::new();

        let stats = read_allocator_stats();
        let total_kb = (stats.num_total_pages * 4096) / 1024;
        let free_kb = (stats.num_free_pages * 4096) / 1024;

        let _ = write!(s, "MemTotal:       {} kB\n", total_kb);
        let _ = write!(s, "MemFree:        {} kB\n", free_kb);
        let _ = write!(s, "MemAvailable:   {} kB\n", free_kb);
        let _ = write!(s, "Buffers:        0 kB\n");
        let _ = write!(s, "Cached:         0 kB\n");
        let _ = write!(s, "SwapTotal:      0 kB\n");
        let _ = write!(s, "SwapFree:       0 kB\n");

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/version ───────────────────────────────────────────────────

pub struct ProcVersionFile;

impl fmt::Debug for ProcVersionFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcVersionFile").finish()
    }
}

impl FileLike for ProcVersionFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        let s = "Kevlar version 0.1.0 (rustc) #1 SMP\n";
        let len = core::cmp::min(s.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&s.as_bytes()[..len])?;
        Ok(len)
    }
}

// ── /proc/cpuinfo ──────────────────────────────────────────────────

pub struct ProcCpuinfoFile;

impl fmt::Debug for ProcCpuinfoFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcCpuinfoFile").finish()
    }
}

impl FileLike for ProcCpuinfoFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        use core::fmt::Write;
        let mut s = alloc::string::String::new();

        let ncpus = kevlar_platform::arch::num_online_cpus();

        #[cfg(target_arch = "x86_64")]
        {
            let freq_hz = kevlar_platform::arch::tsc::frequency_hz();
            let mhz = freq_hz / 1_000_000;
            for i in 0..ncpus {
                let _ = write!(s, "processor\t: {}\n", i);
                let _ = write!(s, "vendor_id\t: GenuineIntel\n");
                let _ = write!(s, "model name\t: QEMU Virtual CPU\n");
                let _ = write!(s, "cpu MHz\t\t: {}.000\n", mhz);
                let _ = write!(s, "cache size\t: 0 KB\n");
                let _ = write!(s, "physical id\t: 0\n");
                let _ = write!(s, "siblings\t: {}\n", ncpus);
                let _ = write!(s, "core id\t\t: {}\n", i);
                let _ = write!(s, "cpu cores\t: {}\n", ncpus);
                let _ = write!(s, "flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2\n");
                let _ = write!(s, "bogomips\t: {}.00\n", mhz * 2);
                let _ = write!(s, "\n");
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            for i in 0..ncpus {
                let _ = write!(s, "processor\t: {}\n", i);
                let _ = write!(s, "BogoMIPS\t: 100.00\n");
                let _ = write!(s, "Features\t: fp asimd evtstrm\n");
                let _ = write!(s, "CPU implementer\t: 0x41\n");
                let _ = write!(s, "CPU architecture: 8\n");
                let _ = write!(s, "CPU variant\t: 0x0\n");
                let _ = write!(s, "CPU part\t: 0xd08\n");
                let _ = write!(s, "CPU revision\t: 3\n");
                let _ = write!(s, "\n");
            }
        }

        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&bytes[..len])?;
        Ok(len)
    }
}

// ── /proc/uptime ───────────────────────────────────────────────────

pub struct ProcUptimeFile;

impl fmt::Debug for ProcUptimeFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcUptimeFile").finish()
    }
}

impl FileLike for ProcUptimeFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        let mono = read_monotonic_clock();
        let secs = mono.msecs() / 1000;
        let frac = (mono.msecs() % 1000) / 10;
        // Format: uptime_secs idle_secs (idle = uptime, single CPU)
        let s = alloc::format!("{}.{:02} {}.{:02}\n", secs, frac, secs, frac);

        let len = core::cmp::min(s.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&s.as_bytes()[..len])?;
        Ok(len)
    }
}

// ── /proc/loadavg ──────────────────────────────────────────────────

pub struct ProcLoadavgFile;

impl fmt::Debug for ProcLoadavgFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcLoadavgFile").finish()
    }
}

impl FileLike for ProcLoadavgFile {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            mode: FileMode::new(S_IFREG | 0o444),
            ..Stat::zeroed()
        })
    }

    fn read(&self, offset: usize, buf: UserBufferMut<'_>, _options: &OpenOptions) -> Result<usize> {
        if offset > 0 {
            return Ok(0);
        }

        let procs = process_count();
        // Format: 1min 5min 15min running/total last_pid
        let s = alloc::format!("0.00 0.00 0.00 1/{} 1\n", procs);

        let len = core::cmp::min(s.len(), buf.len());
        let mut writer = UserBufWriter::from(buf);
        writer.write_bytes(&s.as_bytes()[..len])?;
        Ok(len)
    }
}
