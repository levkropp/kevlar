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

/// Size and layout of `struct epoll_event`.
///
/// Linux `<sys/epoll.h>`:
///
/// ```c
/// struct epoll_event {
///     uint32_t events;
///     epoll_data_t data;  // union, 8 bytes
/// } __EPOLL_PACKED;
/// ```
///
/// `__EPOLL_PACKED` is `__attribute__((packed))` **only on x86_64** —
/// for ABI compatibility with 32-bit x86.  On every other arch
/// (aarch64, riscv, etc.) it is a no-op, so the struct has natural
/// alignment with a 4-byte hole after `events`:
///
/// | arch    | layout                           | size |
/// |---------|----------------------------------|------|
/// | x86_64  | `events(4) | data(8)`            | 12   |
/// | aarch64 | `events(4) | _pad(4) | data(8)`  | 16   |
///
/// Get this wrong and userspace reads `data.ptr` from either the
/// wrong offset (truncated to a partial pointer) or the wrong stride
/// (reads into the next event's header).  Xorg crashes immediately
/// because it stores real pointers in `data.ptr` for its event loop.
#[cfg(target_arch = "x86_64")]
const EPOLL_EVENT_SIZE: usize = 12;
#[cfg(target_arch = "x86_64")]
const EPOLL_DATA_OFFSET: usize = 4;

#[cfg(not(target_arch = "x86_64"))]
const EPOLL_EVENT_SIZE: usize = 16;
#[cfg(not(target_arch = "x86_64"))]
const EPOLL_DATA_OFFSET: usize = 8;

impl EpollEvent {
    /// Deserialize from userspace `struct epoll_event` bytes.
    fn from_bytes(b: &[u8; EPOLL_EVENT_SIZE]) -> EpollEvent {
        let events = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
        let d = EPOLL_DATA_OFFSET;
        let data = u64::from_ne_bytes([
            b[d], b[d + 1], b[d + 2], b[d + 3],
            b[d + 4], b[d + 5], b[d + 6], b[d + 7],
        ]);
        EpollEvent { events, data }
    }

    /// Serialize to userspace `struct epoll_event` bytes.  Pad bytes
    /// (arm64) are zeroed to avoid leaking kernel stack data.
    fn to_bytes(&self) -> [u8; EPOLL_EVENT_SIZE] {
        let mut buf = [0u8; EPOLL_EVENT_SIZE];
        let ev = self.events.to_ne_bytes();
        let da = self.data.to_ne_bytes();
        buf[0..4].copy_from_slice(&ev);
        buf[EPOLL_DATA_OFFSET..EPOLL_DATA_OFFSET + 8].copy_from_slice(&da);
        buf
    }
}

impl<'a> SyscallHandler<'a> {
    /// `epoll_create1(flags)` — create a new epoll instance.
    pub fn sys_epoll_create1(&mut self, flags: c_int) -> Result<isize> {
        let cloexec = (flags & EPOLL_CLOEXEC as i32) != 0;
        let options = OpenOptions::new(false, cloexec);

        let epoll = EpollInstance::new();
        let fd = current_process().opened_files_no_irq().open(
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
        event_ptr: Option<UserVAddr>,
    ) -> Result<isize> {
        // Read the epoll_event from userspace (NULL is valid for EPOLL_CTL_DEL).
        let event = if op != EPOLL_CTL_DEL {
            let ptr = event_ptr.ok_or(Error::new(Errno::EFAULT))?;
            let bytes = ptr.read::<[u8; EPOLL_EVENT_SIZE]>()?;
            Some(EpollEvent::from_bytes(&bytes))
        } else {
            None
        };

        let table = current_process().opened_files_no_irq();

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

        // Fast path: timeout=0 means non-blocking — avoid Arc clone overhead.
        // Safe: poll() on interest files never locks the fd table.
        if timeout == 0 {
            let proc = current_process();

            // Hot-fd cache: skip fd table lookup + downcast when the same
            // epoll fd is polled repeatedly (the common event loop pattern).
            #[cfg(not(feature = "profile-fortress"))]
            if proc.epoll_hot_fd() == epfd.as_int() {
                let ptr = proc.epoll_hot_ptr();
                if !ptr.is_null() {
                    #[allow(unsafe_code)]
                    let epoll = unsafe { &*(ptr as *const EpollInstance) };
                    #[allow(unsafe_code)]
                    let count = unsafe {
                        epoll.collect_ready_to_user_lockfree(events_ptr, maxevents)?
                    };
                    return Ok(count as isize);
                }
            }

            // Exp 1+3: Lock-free fd table + interests when both are unshared.
            #[cfg(not(feature = "profile-fortress"))]
            if Arc::strong_count(proc.opened_files()) == 1 {
                // SAFETY: strong_count == 1 guarantees no concurrent access.
                #[allow(unsafe_code)]
                let table = unsafe { proc.opened_files().get_unchecked() };
                let epoll_file = table.get(epfd)?.as_file()?;
                let epoll = (**epoll_file).as_any().downcast_ref::<EpollInstance>()
                    .ok_or(Error::new(Errno::EINVAL))?;
                // Cache for next call.
                proc.set_epoll_hot(epfd.as_int(), epoll as *const EpollInstance as *mut u8);
                // Exp 3: skip interests lock if epoll instance is unshared.
                if Arc::strong_count(epoll_file) == 1 {
                    #[allow(unsafe_code)]
                    let count = unsafe {
                        epoll.collect_ready_to_user_lockfree(events_ptr, maxevents)?
                    };
                    return Ok(count as isize);
                }
                let count = epoll.collect_ready_to_user(events_ptr, maxevents)?;
                return Ok(count as isize);
            }

            let table = proc.opened_files_no_irq();
            let epoll_file = table.get(epfd)?.as_file()?;
            let epoll = (**epoll_file).as_any().downcast_ref::<EpollInstance>()
                .ok_or(Error::new(Errno::EINVAL))?;
            // Exp 3: also check on the locked fd table path.
            #[cfg(not(feature = "profile-fortress"))]
            if Arc::strong_count(epoll_file) == 1 {
                #[allow(unsafe_code)]
                let count = unsafe {
                    epoll.collect_ready_to_user_lockfree(events_ptr, maxevents)?
                };
                return Ok(count as isize);
            }
            let count = epoll.collect_ready_to_user(events_ptr, maxevents)?;
            return Ok(count as isize);
        }

        // Blocking path: clone Arc so it outlives the fd table lock.
        let epoll_file: Arc<dyn FileLike> = {
            current_process().opened_files_no_irq().get(epfd)?.as_file()?.clone()
        };
        let epoll = (*epoll_file).as_any().downcast_ref::<EpollInstance>()
            .ok_or(Error::new(Errno::EINVAL))?;

        let started_at = crate::timer::read_monotonic_clock();

        // Blocking path: sleep until events are ready or timeout expires.
        let ready_events: Vec<EpollEvent> = POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            if timeout > 0 && started_at.elapsed_msecs() >= timeout as usize {
                return Ok(Some(Vec::new()));
            }

            let mut events = Vec::new();
            let count = epoll.collect_ready(&mut events, maxevents);

            if count > 0 {
                Ok(Some(events))
            } else {
                Ok(None) // Block until something changes.
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
