#!/bin/sh
set -ue

if [ ! -d kevlar ]; then
    git clone https://github.com/nuta/kevlar
fi

cd kevlar
git pull

curl https://sh.rustup.rs -sSf | sh -s -- -y
source $HOME/.cargo/env

rustup override set nightly
rustup component add rust-src

mkdir build
touch build/kevlar.initramfs
make src-docs
mv target/doc ../public
