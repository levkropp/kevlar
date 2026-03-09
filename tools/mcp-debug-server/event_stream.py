#!/usr/bin/env python3
"""
Debug event stream consumer for the Kevlar kernel.

Reads JSONL debug events (lines prefixed with "DBG ") from:
- A file (QEMU serial log or ISA debugcon output)
- A named pipe / PTY
- Stdin

Maintains an indexed ring buffer for efficient querying by the MCP server.
"""

import json
import threading
import time
from collections import defaultdict, deque
from pathlib import Path
from typing import Optional


class DebugEventStream:
    """Reads, indexes, and queries structured kernel debug events."""

    def __init__(self, capacity: int = 50000):
        self.events: deque = deque(maxlen=capacity)
        self.lock = threading.Lock()

        # Indexes for fast lookup.
        self._by_pid: dict[int, deque] = defaultdict(lambda: deque(maxlen=5000))
        self._by_type: dict[str, deque] = defaultdict(lambda: deque(maxlen=5000))
        self._syscall_errors: dict[str, dict[str, int]] = defaultdict(
            lambda: defaultdict(int)
        )
        self._canary_corruptions: list[dict] = []
        self._panics: list[dict] = []
        self._usercopy_faults: list[dict] = []

    def ingest_line(self, line: str) -> Optional[dict]:
        """Parse a single line. Returns the event dict if it was a DBG line."""
        line = line.rstrip("\n\r")
        if not line.startswith("DBG "):
            return None

        try:
            event = json.loads(line[4:])
        except json.JSONDecodeError:
            return None

        with self.lock:
            self.events.append(event)

            event_type = event.get("type", "")
            self._by_type[event_type].append(event)

            pid = event.get("pid")
            if pid is not None:
                self._by_pid[pid].append(event)

            # Index syscall errors.
            if event_type == "syscall_exit" and "errno" in event:
                name = event.get("name", "?")
                errno = event["errno"]
                self._syscall_errors[name][errno] += 1

            # Track canary corruptions.
            if event_type == "canary_check" and event.get("corrupted"):
                self._canary_corruptions.append(event)

            # Track panics.
            if event_type == "panic":
                self._panics.append(event)

            # Track usercopy faults (high-value debug events).
            if event_type == "usercopy_fault":
                self._usercopy_faults.append(event)

            # Track usercopy trace dumps (emitted on corruption/fault).
            if event_type == "ucopy_trace_dump":
                self._usercopy_faults.append(event)  # Index with faults for easy retrieval

        return event

    def ingest_file(self, path: str, follow: bool = False) -> None:
        """Read events from a file. If follow=True, tail -f style."""
        p = Path(path)
        if not p.exists():
            return

        with open(p) as f:
            while True:
                line = f.readline()
                if line:
                    self.ingest_line(line)
                elif follow:
                    time.sleep(0.1)
                else:
                    break

    def ingest_file_background(self, path: str) -> threading.Thread:
        """Start a background thread that tails the debug log file."""
        t = threading.Thread(
            target=self.ingest_file, args=(path, True), daemon=True
        )
        t.start()
        return t

    # ── Query methods ──

    def query(
        self,
        pid: Optional[int] = None,
        event_type: Optional[str] = None,
        last_n: int = 50,
    ) -> list[dict]:
        """Query events with optional filters."""
        with self.lock:
            if pid is not None:
                source = self._by_pid.get(pid, deque())
            elif event_type is not None:
                source = self._by_type.get(event_type, deque())
            else:
                source = self.events

            result = list(source)[-last_n:]

            if pid is not None and event_type is not None:
                result = [e for e in result if e.get("type") == event_type]

            return result

    def get_syscall_trace(
        self,
        pid: Optional[int] = None,
        last_n: int = 50,
        name_filter: Optional[str] = None,
    ) -> list[dict]:
        """Get recent syscall entry/exit pairs."""
        with self.lock:
            if pid is not None:
                source = self._by_pid.get(pid, deque())
            else:
                source = self.events

            syscalls = [
                e
                for e in source
                if e.get("type") in ("syscall_entry", "syscall_exit")
            ]

            if name_filter:
                syscalls = [
                    e for e in syscalls if name_filter in e.get("name", "")
                ]

            return syscalls[-last_n:]

    def get_failed_syscalls(
        self, errno: Optional[str] = None, last_n: int = 100
    ) -> list[dict]:
        """Get syscall exit events that returned an error."""
        with self.lock:
            failures = [
                e
                for e in self._by_type.get("syscall_exit", deque())
                if "errno" in e
            ]
            if errno:
                failures = [e for e in failures if e.get("errno") == errno]
            return failures[-last_n:]

    def get_syscall_error_summary(self) -> dict:
        """Aggregate: {syscall_name: {errno: count}}."""
        with self.lock:
            return {
                name: dict(errnos)
                for name, errnos in self._syscall_errors.items()
            }

    def get_canary_corruptions(self) -> list[dict]:
        """All detected canary corruptions."""
        with self.lock:
            return list(self._canary_corruptions)

    def get_panics(self) -> list[dict]:
        """All panic events."""
        with self.lock:
            return list(self._panics)

    def get_signal_history(
        self, pid: Optional[int] = None, last_n: int = 50
    ) -> list[dict]:
        """Get signal delivery events."""
        with self.lock:
            signals = list(self._by_type.get("signal", deque()))
            if pid is not None:
                signals = [e for e in signals if e.get("pid") == pid]
            return signals[-last_n:]

    def get_fault_history(self, last_n: int = 50) -> list[dict]:
        """Get page fault, user fault, and usercopy fault events."""
        with self.lock:
            faults = list(self._by_type.get("page_fault", deque()))
            faults += list(self._by_type.get("user_fault", deque()))
            faults += list(self._by_type.get("usercopy_fault", deque()))
            faults.sort(
                key=lambda e: self.events.index(e)
                if e in self.events
                else 0
            )
            return faults[-last_n:]

    def get_usercopy_faults(self) -> list[dict]:
        """All usercopy fault events (faults during copy_to/from_user)."""
        with self.lock:
            return list(self._usercopy_faults)

    def get_usercopy_trace(
        self, pid: Optional[int] = None, last_n: int = 100
    ) -> list[dict]:
        """Get usercopy trace events (copy_to/from_user calls)."""
        with self.lock:
            copies = list(self._by_type.get("usercopy", deque()))
            if pid is not None:
                copies = [e for e in copies if e.get("pid") == pid]
            return copies[-last_n:]

    def get_signal_stack_writes(
        self, pid: Optional[int] = None, last_n: int = 50
    ) -> list[dict]:
        """Get signal stack write events."""
        with self.lock:
            writes = list(self._by_type.get("signal_stack_write", deque()))
            if pid is not None:
                writes = [e for e in writes if e.get("pid") == pid]
            return writes[-last_n:]

    def get_process_events(self, last_n: int = 50) -> list[dict]:
        """Get process lifecycle events (fork, exec, exit)."""
        with self.lock:
            procs = []
            for t in ("process_fork", "process_exec", "process_exit"):
                procs += list(self._by_type.get(t, deque()))
            return procs[-last_n:]

    def get_summary(self) -> dict:
        """Executive summary of the debug session."""
        with self.lock:
            return {
                "total_events": len(self.events),
                "total_syscall_entries": len(
                    self._by_type.get("syscall_entry", deque())
                ),
                "total_syscall_exits": len(
                    self._by_type.get("syscall_exit", deque())
                ),
                "failed_syscalls": sum(
                    sum(v.values())
                    for v in self._syscall_errors.values()
                ),
                "signals_delivered": len(
                    self._by_type.get("signal", deque())
                ),
                "page_faults": len(
                    self._by_type.get("page_fault", deque())
                ),
                "user_faults": len(
                    self._by_type.get("user_fault", deque())
                ),
                "canary_corruptions": len(self._canary_corruptions),
                "usercopy_faults": len(self._usercopy_faults),
                "usercopy_events": len(
                    self._by_type.get("usercopy", deque())
                ),
                "signal_stack_writes": len(
                    self._by_type.get("signal_stack_write", deque())
                ),
                "panics": len(self._panics),
                "unique_pids": len(self._by_pid),
                "error_summary": {
                    name: dict(errnos)
                    for name, errnos in self._syscall_errors.items()
                },
            }
