#!/bin/bash
# Wrapper script for make on Windows
# Ensures cargo and docker are in PATH

# Add cargo to PATH
export PATH="$HOME/.cargo/bin:$PATH"

# Add Docker to PATH (always, for consistency)
export PATH="/c/Program Files/Docker/Docker/resources/bin:$PATH"

# Export for Python scripts
export DOCKER="C:/Program Files/Docker/Docker/resources/bin/docker.exe"

# Run make with all arguments
exec make "$@"
