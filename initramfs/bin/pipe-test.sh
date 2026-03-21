#!/bin/sh
echo "static pipe test"
echo "ls | head:"
ls /bin | head -3
echo "echo | cat:"
echo "hello from pipe" | cat
echo "done"
