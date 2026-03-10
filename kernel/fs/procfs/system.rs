// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! System-wide /proc files: /proc/mounts, /proc/filesystems, /proc/cmdline,
//! /proc/stat, /proc/meminfo, /proc/version.
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
