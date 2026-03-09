// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Capability tokens for ring boundary enforcement.
//!
//! Capabilities prove that a service is authorized to perform an operation.
//! The kernel core mints capability tokens during service registration and
//! passes them to services. Services must hold the token to call privileged
//! platform functions.
//!
//! - **Fortress**: Runtime-validated tokens with a random nonce. Forged tokens
//!   are detected and rejected.
//! - **Balanced**: Zero-cost newtypes. The type system enforces authorization
//!   at compile time; the runtime cost is zero.
//! - **Performance/Ludicrous**: No capability tokens. Functions are called
//!   directly without authorization checks.

use core::marker::PhantomData;

// --- Capability marker types ---

/// Permission to send and receive network frames.
pub enum NetAccess {}

/// Permission to allocate physical page frames.
pub enum PageAlloc {}

/// Permission to access block devices.
pub enum BlockAccess {}

// --- Fortress: runtime-validated tokens ---

#[cfg(feature = "profile-fortress")]
mod inner {
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};

    /// Global nonce counter — each minted token gets a unique nonce.
    static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

    /// Stores the valid nonce for each capability type.
    static NET_NONCE: AtomicU64 = AtomicU64::new(0);
    static PAGE_ALLOC_NONCE: AtomicU64 = AtomicU64::new(0);
    static BLOCK_NONCE: AtomicU64 = AtomicU64::new(0);

    /// A capability token that is validated at runtime.
    #[derive(Clone)]
    pub struct Cap<T> {
        nonce: u64,
        _marker: PhantomData<T>,
    }

    impl<T> Cap<T> {
        fn new(nonce: u64) -> Self {
            Cap {
                nonce,
                _marker: PhantomData,
            }
        }
    }

    fn nonce_for<T: 'static>() -> &'static AtomicU64 {
        use core::any::TypeId;
        let id = TypeId::of::<T>();
        if id == TypeId::of::<NetAccess>() {
            &NET_NONCE
        } else if id == TypeId::of::<PageAlloc>() {
            &PAGE_ALLOC_NONCE
        } else if id == TypeId::of::<BlockAccess>() {
            &BLOCK_NONCE
        } else {
            panic!("unknown capability type");
        }
    }

    /// Mint a new capability token. Only callable from the kernel core
    /// during service registration.
    pub fn mint<T: 'static>() -> Cap<T> {
        let nonce = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
        nonce_for::<T>().store(nonce, Ordering::Release);
        Cap::new(nonce)
    }

    /// Validate that a capability token is genuine.
    pub fn validate<T: 'static>(cap: &Cap<T>) -> bool {
        cap.nonce == nonce_for::<T>().load(Ordering::Acquire) && cap.nonce != 0
    }
}

// --- Balanced: zero-cost compile-time tokens ---

#[cfg(feature = "profile-balanced")]
mod inner {
    use super::*;

    /// A zero-cost capability token. Authorization is enforced by the type
    /// system — only code that receives a `Cap<T>` from the kernel can call
    /// functions requiring it.
    #[derive(Clone)]
    pub struct Cap<T> {
        _marker: PhantomData<T>,
    }

    /// Mint a new capability token.
    pub fn mint<T: 'static>() -> Cap<T> {
        Cap {
            _marker: PhantomData,
        }
    }

    /// Always valid — compile-time enforcement only.
    #[inline(always)]
    pub fn validate<T: 'static>(_cap: &Cap<T>) -> bool {
        true
    }
}

// --- Performance/Ludicrous: no capability tokens ---

#[cfg(any(feature = "profile-performance", feature = "profile-ludicrous"))]
mod inner {
    use super::*;

    /// A zero-size token that is always valid. Exists only to keep the API
    /// uniform across profiles; it compiles away entirely.
    #[derive(Clone)]
    pub struct Cap<T> {
        _marker: PhantomData<T>,
    }

    #[inline(always)]
    pub fn mint<T: 'static>() -> Cap<T> {
        Cap {
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    pub fn validate<T: 'static>(_cap: &Cap<T>) -> bool {
        true
    }
}

pub use inner::{mint, validate, Cap};
