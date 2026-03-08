#!/bin/sh
set -ue

if [ ! -d kevlar ]; then
    git clone https://github.com/nuta/kevlar
fi

cd kevlar
git pull
make docs
mv build/docs ../public/docs
