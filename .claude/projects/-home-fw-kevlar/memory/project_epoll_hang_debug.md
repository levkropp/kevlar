---
name: Epoll hang root cause analysis
description: Debugging notes for epoll_level contract test hang - pipe read blocks after epoll_ctl(ADD)
type: project
---

## epoll_level hang — Root cause narrowed (2026-03-18)

The `events.epoll_level` contract test hangs. Through systematic bisection:

1. **Plain pipe read works** — `pipe() → write("X") → read()` succeeds with no issues
2. **epoll_ctl(ADD) breaks subsequent pipe reads** — after `epoll_create1() + epoll_ctl(ADD, pipe_read_fd)`, ANY `read(pipe_read_fd)` blocks indefinitely, even though the pipe has data
3. **The hang is NOT in epoll_wait** — it's in the `read()` syscall itself
4. **The blocking epoll_wait(timeout=100) works** — it returns correctly when data is present
5. **Consistent across all profiles** — fortress, balanced, performance, ludicrous all hang

**Why:** `epoll_ctl(ADD)` clones the `Arc<dyn FileLike>` (PipeReader) into the Interest struct. This increments the Arc strong_count. Later, `PipeReader::read()` fast path should find data via `pipe.buf.pop_slice()`. Since it blocks, either:
- The pipe's inner SpinLock is somehow held (deadlock)
- The RingBuffer is unexpectedly empty (data corruption)
- The `get_opened_file_by_fd()` returns a different OpenedFile that wraps a different PipeReader

**How to apply:** The most likely cause is a lock ordering or deadlock issue introduced by having two Arc references to the same PipeReader (one in the fd table, one in the epoll Interest). Focus investigation on whether `lock_no_irq()` on the pipe inner can deadlock when the epoll instance also holds a reference.

**Test binary for repro:**
```c
#include <sys/epoll.h>
#include <unistd.h>
int main(void) {
    int ep = epoll_create1(0);
    int fds[2]; pipe(fds);
    struct epoll_event ev = {.events = EPOLLIN, .data.fd = fds[0]};
    epoll_ctl(ep, EPOLL_CTL_ADD, fds[0], &ev);
    write(fds[1], "abc", 3);
    char buf; read(fds[0], &buf, 1); // HANGS HERE
    return 0;
}
```
