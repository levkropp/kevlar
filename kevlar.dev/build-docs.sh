#!/bin/sh
set -ue

if [ ! -d kevlar ]; then
    git clone https://github.com/levkropp/kevlar
fi

cd kevlar
git pull
make docs
mv build/docs ../public/docs
