# Logging

Kevlar uses the `log` crate. By default, `trace!` and `debug!` messages are
suppressed. Control verbosity with `LOG=` on the make command line:

```
make run LOG=trace                           # All trace messages
make run LOG="kevlar_kernel::fs=trace"       # Traces only from the fs module
make run LOG="kevlar_kernel::net=debug"      # Debug level for networking
```

The module path prefix is `kevlar_kernel::` (matching the kernel crate package name).

## Secondary serial port

When kernel output is noisy, redirect it to a file via the secondary serial port:

```
make run LOG=trace LOG_SERIAL="file:/tmp/kevlar-debug.log"
```

Or append to an existing file:

```
make run LOG=trace \
    QEMU_ARGS="-chardev file,id=uart1,path=/tmp/kevlar.log,logappend=on" \
    LOG_SERIAL="chardev:uart1"
```

## Structured debug events

The `debug=` kernel parameter enables structured JSONL debug events (separate from
the log level). See [Debugging 101](hacking/debugging-101.md) for details.
