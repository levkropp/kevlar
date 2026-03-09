#!/usr/bin/env python3
"""
GDB/MI bridge for live kernel state inspection.

Connects to QEMU's GDB stub and provides structured access to kernel
data structures. Used by the MCP server for live debugging tools.

This module reads kernel memory via GDB, resolves symbols, and interprets
Rust data structures (BTreeMap, Vec, Arc, etc.) to extract process tables,
VMA maps, and other kernel state.
"""

import re
import subprocess
import json
from pathlib import Path
from typing import Optional


class GDBBridge:
    """Connects to QEMU's GDB stub for kernel state inspection."""

    def __init__(
        self,
        gdb_port: int = 7789,
        elf_path: str = "kevlar.x64.elf",
        gdb_binary: str = "gdb",
    ):
        self.gdb_port = gdb_port
        self.elf_path = elf_path
        self.gdb_binary = gdb_binary
        self._connected = False
        self._gdb = None

    def connect(self) -> bool:
        """Try to connect to the GDB stub. Returns True on success."""
        try:
            from pygdbmi.gdbcontroller import GdbController

            self._gdb = GdbController(
                [self.gdb_binary, "--interpreter=mi3", "-q", self.elf_path]
            )
            resp = self._gdb.write(
                f"-target-select remote localhost:{self.gdb_port}", timeout_sec=5
            )
            for r in resp:
                if r.get("message") == "connected":
                    self._connected = True
                    return True
            self._connected = True  # Some GDB versions don't emit "connected"
            return True
        except Exception as e:
            self._connected = False
            return False

    @property
    def connected(self) -> bool:
        return self._connected

    def _exec(self, cmd: str, timeout: int = 10) -> list[dict]:
        """Execute a GDB/MI command and return parsed responses."""
        if not self._gdb:
            return []
        try:
            return self._gdb.write(cmd, timeout_sec=timeout)
        except Exception:
            self._connected = False
            return []

    def read_memory(
        self, address: int, length: int, fmt: str = "hex"
    ) -> Optional[dict]:
        """Read memory from the kernel. Returns formatted data."""
        resp = self._exec(f"-data-read-memory-bytes {address:#x} {length}")
        for r in resp:
            if r.get("message") == "done" and "payload" in r:
                memory = r["payload"].get("memory", [])
                if memory:
                    hex_data = memory[0].get("contents", "")
                    raw = bytes.fromhex(hex_data)
                    if fmt == "hex":
                        return {"address": address, "hex": hex_data, "length": len(raw)}
                    elif fmt == "ascii":
                        return {
                            "address": address,
                            "ascii": raw.decode("ascii", errors="replace"),
                            "length": len(raw),
                        }
                    elif fmt == "u64_array":
                        u64s = []
                        for i in range(0, len(raw), 8):
                            if i + 8 <= len(raw):
                                u64s.append(int.from_bytes(raw[i : i + 8], "little"))
                        return {"address": address, "u64s": u64s}
        return None

    def get_registers(self) -> Optional[dict]:
        """Get current CPU register state."""
        resp = self._exec("-data-list-register-values x")
        for r in resp:
            if r.get("message") == "done" and "payload" in r:
                regs = {}
                for entry in r["payload"].get("register-values", []):
                    regs[f"r{entry['number']}"] = entry.get("value", "?")
                return regs
        return None

    def get_backtrace(self) -> list[dict]:
        """Get the current backtrace."""
        resp = self._exec("-stack-list-frames")
        frames = []
        for r in resp:
            if r.get("message") == "done" and "payload" in r:
                for f in r["payload"].get("stack", []):
                    frame = f.get("frame", f)
                    frames.append(
                        {
                            "level": frame.get("level", "?"),
                            "addr": frame.get("addr", "?"),
                            "func": frame.get("func", "?"),
                            "file": frame.get("file", ""),
                            "line": frame.get("line", ""),
                        }
                    )
        return frames

    def resolve_symbol(self, address: int) -> Optional[dict]:
        """Resolve an address to a symbol name."""
        resp = self._exec(f"info symbol {address:#x}")
        for r in resp:
            payload = r.get("payload", "")
            if payload and "in section" in payload:
                # Parse: "symbol_name + offset in section .text"
                match = re.match(r"(\S+)\s*\+\s*(\d+)\s+in section", payload)
                if match:
                    return {
                        "address": address,
                        "symbol": match.group(1),
                        "offset": int(match.group(2)),
                    }
                match = re.match(r"(\S+)\s+in section", payload)
                if match:
                    return {"address": address, "symbol": match.group(1), "offset": 0}
        return {"address": address, "symbol": "(unknown)", "offset": 0}

    def evaluate(self, expr: str) -> Optional[str]:
        """Evaluate a GDB expression."""
        resp = self._exec(f"-data-evaluate-expression \"{expr}\"")
        for r in resp:
            if r.get("message") == "done" and "payload" in r:
                return r["payload"].get("value")
        return None

    def continue_execution(self) -> None:
        """Resume kernel execution."""
        self._exec("-exec-continue")

    def interrupt(self) -> None:
        """Stop the kernel (Ctrl+C equivalent)."""
        self._exec("-exec-interrupt")

    def disconnect(self) -> None:
        """Disconnect from GDB stub."""
        if self._gdb:
            try:
                self._gdb.exit()
            except Exception:
                pass
            self._gdb = None
            self._connected = False


class SymbolResolver:
    """Resolves addresses using the .symbols file (offline, no GDB needed)."""

    def __init__(self, symbols_path: str):
        self.symbols: list[tuple[int, str]] = []
        self._load(symbols_path)

    def _load(self, path: str) -> None:
        p = Path(path)
        if not p.exists():
            return
        for line in p.read_text().splitlines():
            parts = line.strip().split(None, 1)
            if len(parts) == 2:
                try:
                    addr = int(parts[0], 16)
                    name = parts[1].strip()
                    self.symbols.append((addr, name))
                except ValueError:
                    continue
        self.symbols.sort(key=lambda x: x[0])

    def resolve(self, address: int) -> dict:
        """Binary search for the symbol containing this address."""
        if not self.symbols:
            return {"address": address, "symbol": "(no symbols)", "offset": 0}

        lo, hi = 0, len(self.symbols) - 1
        while lo <= hi:
            mid = (lo + hi) // 2
            if self.symbols[mid][0] <= address:
                lo = mid + 1
            else:
                hi = mid - 1

        if hi >= 0:
            addr, name = self.symbols[hi]
            return {"address": address, "symbol": name, "offset": address - addr}

        return {"address": address, "symbol": "(unknown)", "offset": 0}
