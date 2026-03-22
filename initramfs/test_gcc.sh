#!/bin/sh
echo "=== GCC TEST ==="
echo 'int main(){return 42;}' > /root/t.c
gcc -o /root/t /root/t.c 2>&1
echo "gcc=$?"
ls -la /root/t 2>&1
echo "ls=$?"
/root/t 2>&1
echo "run=$?"
echo "=== DONE ==="
poweroff -f
