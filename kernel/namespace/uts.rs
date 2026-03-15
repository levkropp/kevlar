// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! UTS namespace: hostname and domainname isolation.
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::result::{Errno, Result};
use kevlar_platform::spinlock::SpinLock;

pub struct UtsNamespace {
    hostname: SpinLock<[u8; 65]>,
    hostname_len: AtomicUsize,
    domainname: SpinLock<[u8; 65]>,
    domainname_len: AtomicUsize,
}

impl UtsNamespace {
    pub fn new() -> UtsNamespace {
        UtsNamespace {
            hostname: SpinLock::new([0u8; 65]),
            hostname_len: AtomicUsize::new(0),
            domainname: SpinLock::new([0u8; 65]),
            domainname_len: AtomicUsize::new(0),
        }
    }

    /// Clone this namespace (copy hostname/domainname).
    pub fn clone_ns(&self) -> UtsNamespace {
        let mut new_host = [0u8; 65];
        let host = self.hostname.lock();
        new_host.copy_from_slice(&*host);
        let host_len = self.hostname_len.load(Ordering::Relaxed);
        drop(host);

        let mut new_dom = [0u8; 65];
        let dom = self.domainname.lock();
        new_dom.copy_from_slice(&*dom);
        let dom_len = self.domainname_len.load(Ordering::Relaxed);
        drop(dom);

        UtsNamespace {
            hostname: SpinLock::new(new_host),
            hostname_len: AtomicUsize::new(host_len),
            domainname: SpinLock::new(new_dom),
            domainname_len: AtomicUsize::new(dom_len),
        }
    }

    pub fn get_hostname(&self) -> Vec<u8> {
        let host = self.hostname.lock();
        let len = self.hostname_len.load(Ordering::Relaxed);
        host[..len].to_vec()
    }

    pub fn set_hostname(&self, name: &[u8]) -> Result<()> {
        if name.len() > 64 {
            return Err(Errno::EINVAL.into());
        }
        let mut host = self.hostname.lock();
        host[..name.len()].copy_from_slice(name);
        if name.len() < 65 {
            host[name.len()] = 0;
        }
        self.hostname_len.store(name.len(), Ordering::Relaxed);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_domainname(&self) -> Vec<u8> {
        let dom = self.domainname.lock();
        let len = self.domainname_len.load(Ordering::Relaxed);
        dom[..len].to_vec()
    }

    pub fn set_domainname(&self, name: &[u8]) -> Result<()> {
        if name.len() > 64 {
            return Err(Errno::EINVAL.into());
        }
        let mut dom = self.domainname.lock();
        dom[..name.len()].copy_from_slice(name);
        if name.len() < 65 {
            dom[name.len()] = 0;
        }
        self.domainname_len.store(name.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Write hostname directly into a utsname buffer field (zero-copy, no heap).
    /// Uses lock_no_irq since UTS is never accessed from interrupt context.
    #[inline]
    pub fn write_hostname_into(&self, utsname: &mut [u8; 390], field_idx: usize) {
        let host = self.hostname.lock_no_irq();
        let len = self.hostname_len.load(Ordering::Relaxed).min(64);
        let offset = field_idx * 65;
        utsname[offset..offset + len].copy_from_slice(&host[..len]);
    }

    /// Write domainname directly into a utsname buffer field (zero-copy, no heap).
    #[inline]
    pub fn write_domainname_into(&self, utsname: &mut [u8; 390], field_idx: usize) {
        let dom = self.domainname.lock_no_irq();
        let len = self.domainname_len.load(Ordering::Relaxed).min(64);
        let offset = field_idx * 65;
        utsname[offset..offset + len].copy_from_slice(&dom[..len]);
    }
}
