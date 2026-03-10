// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! epoll_create1(2), epoll_ctl(2), epoll_wait(2) syscall handlers.
//!
//! Provenance: Own (Linux epoll(7) man pages).
use crate::{
    ctypes::c_int,
    fs::{
        epoll::{EpollEvent, EpollInstance, EPOLL_CLOEXEC, EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLL_CTL_MOD},
        inode::{FileLike, INode},
        opened_file::{Fd, OpenOptions, PathComponent},
    },
    poll::POLL_WAIT_QUEUE,
    prelude::*,
    process::current_process,
    syscalls::SyscallHandler,
};
use alloc::vec::Vec;
use kevlar_platform::address::UserVAddr;

/// Size of `struct epoll_event` (u32 events + u64 data, packed = 12 bytes).
const EPOLL_EVENT_SIZE: usize = 12;

impl EpollEvent {
    /// Deserialize from a 12-byte little-endian buffer.
    fn from_bytes(b: &[u8; EPOLL_EVENT_SIZE]) -> EpollEvent {
        let events = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
        let data = u64::from_ne_bytes([b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11]]);
        EpollEvent { events, data }
    }

    /// Serialize to a 12-byte little-endian buffer.
    fn to_bytes(&self) -> [u8; EPOLL_EVENT_SIZE] {
        let mut buf = [0u8; EPOLL_EVENT_SIZE];
        let ev = self.events.to_ne_bytes();
        let da = self.data.to_ne_bytes();
        buf[0..4].copy_from_slice(&ev);
        buf[4..12].copy_from_slice(&da);
        buf
    }
}

impl<'a> SyscallHandler<'a> {
    /// `epoll_create1(flags)` — create a new epoll instance.
    pub fn sys_epoll_create1(&mut self, flags: c_int) -> Result<isize> {
        let cloexec = (flags & EPOLL_CLOEXEC as i32) != 0;
        let options = OpenOptions::new(false, cloexec);

        let epoll = EpollInstance::new();
        let fd = current_process().opened_files().lock().open(
            PathComponent::new_anonymous(INode::FileLike(epoll as Arc<dyn FileLike>)),
            options,
        )?;
        Ok(fd.as_int() as isize)
    }

    /// `epoll_ctl(epfd, op, fd, event)` — add/modify/delete interest.
    pub fn sys_epoll_ctl(
        &mut self,
        epfd: Fd,
        op: c_int,
        fd: Fd,
        event_ptr: UserVAddr,
    ) -> Result<isize> {
        // Read the epoll_event from userspace.
        let event = if op != EPOLL_CTL_DEL {
            let bytes = event_ptr.read::<[u8; EPOLL_EVENT_SIZE]>()?;
            Some(EpollEvent::from_bytes(&bytes))
        } else {
            None
        };

        let table = current_process().opened_files().lock();

        // Get the epoll instance from the epoll fd.
        let epoll_file = table.get(epfd)?.as_file()?;
        // Deref through Arc so as_any() dispatches via the dyn FileLike vtable
        // to the concrete type, not the blanket Downcastable impl on Arc itself.
        let epoll = (**epoll_file).as_any().downcast_ref::<EpollInstance>()
            .ok_or(Error::new(Errno::EINVAL))?;

        match op {
            EPOLL_CTL_ADD => {
                // Get the target file to watch.
                let target_file = table.get(fd)?.as_file()?.clone();
                epoll.add(fd, target_file, event.as_ref().unwrap())?;
            }
            EPOLL_CTL_MOD => {
                epoll.modify(fd, event.as_ref().unwrap())?;
            }
            EPOLL_CTL_DEL => {
                epoll.delete(fd)?;
            }
            _ => return Err(Errno::EINVAL.into()),
        }

        Ok(0)
    }

    /// `epoll_wait(epfd, events, maxevents, timeout)` — wait for events.
    pub fn sys_epoll_wait(
        &mut self,
        epfd: Fd,
        events_ptr: UserVAddr,
        maxevents: c_int,
        timeout: c_int,
    ) -> Result<isize> {
        if maxevents <= 0 {
            return Err(Errno::EINVAL.into());
        }
        let maxevents = maxevents as usize;

        // Get the epoll instance (clone the Arc to outlive the table lock).
        let epoll_file: Arc<dyn FileLike> = {
            let table = current_process().opened_files().lock();
            table.get(epfd)?.as_file()?.clone()
        };
        let epoll = (*epoll_file).as_any().downcast_ref::<EpollInstance>()
            .ok_or(Error::new(Errno::EINVAL))?;

        let started_at = crate::timer::read_monotonic_clock();

        // Use the existing poll wait queue for sleeping.
        let ready_events: Vec<EpollEvent> = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            // Check timeout.
            if timeout > 0 && started_at.elapsed_msecs() >= timeout as usize {
                return Ok(Some(Vec::new()));
            }

            // Poll all interested fds.
            let mut events = Vec::new();
            let count = epoll.collect_ready(&mut events, maxevents);

            if count > 0 {
                Ok(Some(events))
            } else if timeout == 0 {
                // Non-blocking: return immediately with 0 events.
                Ok(Some(Vec::new()))
            } else {
                // Block until something changes.
                Ok(None)
            }
        })?;

        // Write results to userspace.
        let count = ready_events.len();
        for (i, event) in ready_events.iter().enumerate() {
            let offset = i * EPOLL_EVENT_SIZE;
            let dest = events_ptr.add(offset);
            dest.write_bytes(&event.to_bytes())?;
        }

        Ok(count as isize)
    }
}
