// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::user_buffer::UserBufferMut;
use crate::{prelude::*, user_buffer::UserBufWriter};

pub fn read_secure_random(buf: UserBufferMut<'_>) -> Result<usize> {
    UserBufWriter::from(buf).write_with(|slice| {
        #[cfg(target_arch = "x86_64")]
        {
            let valid = unsafe { x86::random::rdrand_slice(slice) };
            if valid {
                return Ok(slice.len());
            }
            warn_once!("RDRAND returned invalid data");
        }

        #[cfg(target_arch = "aarch64")]
        {
            // ARM64: use RNDR if available, otherwise fill with counter-based values.
            // TODO: Implement proper CRNG.
            for (i, byte) in slice.iter_mut().enumerate() {
                let counter = kevlar_runtime::arch::read_clock_counter();
                *byte = (counter.wrapping_add(i as u64) & 0xFF) as u8;
            }
            return Ok(slice.len());
        }

        #[allow(unreachable_code)]
        Ok(0)
    })
}

pub fn read_insecure_random(buf: UserBufferMut<'_>) -> Result<usize> {
    // TODO:
    read_secure_random(buf)
}
