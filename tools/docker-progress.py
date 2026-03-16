#!/usr/bin/env python3
"""Filter Docker build output into a compact progress display.

Collapses "Step N/M : ..." lines into a single updating line.
Passes through non-step lines (errors, warnings) verbatim.
"""
import sys

total = None
for line in sys.stdin:
    line = line.rstrip('\n')
    if line.startswith('Step '):
        # Parse "Step N/M : description"
        parts = line.split(' ', 3)
        if len(parts) >= 4 and '/' in parts[1]:
            n, m = parts[1].split('/')
            desc = parts[3] if len(parts) > 3 else ''
            total = m
            # Truncate long descriptions
            if len(desc) > 60:
                desc = desc[:57] + '...'
            sys.stderr.write(f'\r\033[K  docker [{n}/{m}] {desc}')
            sys.stderr.flush()
            continue
    elif line.startswith(' ---> '):
        # Cache hit or layer ID — skip
        continue
    elif line.startswith('Sending build context'):
        sys.stderr.write(f'\r\033[K  docker: {line}\n')
        sys.stderr.flush()
        continue
    elif line.startswith('Successfully built') or line.startswith('Successfully tagged'):
        sys.stderr.write(f'\r\033[K  docker: {line}\n')
        sys.stderr.flush()
        continue
    elif line.strip() == '':
        continue
    else:
        # Pass through other lines (errors, RUN output, etc.)
        # Clear the progress line first
        sys.stderr.write(f'\r\033[K')
        print(line, flush=True)

# Final newline after progress
sys.stderr.write('\r\033[K')
sys.stderr.flush()
