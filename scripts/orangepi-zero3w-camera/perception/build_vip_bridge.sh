#!/bin/sh
set -eu
: "${VIPLITE_INCLUDE:?Set VIPLITE_INCLUDE to the vendor SDK v2.0/inc directory}"
: "${1:?Usage: build_vip_bridge.sh /path/to/libmicroduck-vip.so}"
src=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cc -std=c11 -O2 -Wall -Wextra -Werror -fPIC -shared \
  -Wl,-z,defs -I"$VIPLITE_INCLUDE" "$src/vip_bridge.c" -lNBGlinker -lVIPhal -lm -o "$1"
