// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
fn main() {
    // Tell Cargo to recompile when these environment variables change,
    // since they are read via option_env!() / env!() in the kernel.
    println!("cargo::rerun-if-env-changed=INIT_SCRIPT");
    println!("cargo::rerun-if-env-changed=INITRAMFS_PATH");
}
