#!/bin/sh
# Install the verified A733 camera stack separately from distro GStreamer.
set -eu

usage() {
  echo 'Usage: install.sh [ISP602_DEB GST_GOOD_DEB]'
  echo 'Run on aarch64 Linux. Optional DESTDIR stages files without replacing the running setup.'
}
if [ "${1:-}" = --help ]; then usage; exit 0; fi
if [ "$#" -ne 0 ] && [ "$#" -ne 2 ]; then usage >&2; exit 2; fi
case "$(uname -s):$(uname -m)" in
  Linux:aarch64) ;;
  *) echo 'Build and install on the Orange Pi Zero 3W (aarch64 Linux).' >&2; exit 1;;
esac
destination=${DESTDIR:-}
case "$destination" in
  ''|/*) ;;
  *) echo 'DESTDIR must be an absolute path.' >&2; exit 2;;
esac
if [ -z "$destination" ] && [ "$(id -u)" -ne 0 ]; then
  echo 'Run as root, or use DESTDIR for a staged install.' >&2
  exit 1
fi
for command in gcc dpkg-deb sha256sum install mktemp; do
  command -v "$command" >/dev/null || { echo "Missing: $command" >&2; exit 1; }
done

source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The Orange Pi package with the same filename lacks IMX219 tuning. Pin the
# Radxa package bytes that produced correct colour on this A733/6.6 setup.
revision=43fe44a304e8c06c6275769c4731ff82265325a7
base="https://raw.githubusercontent.com/radxa/allwininer-debian/$revision/packages/arm64"
if [ "$#" -eq 2 ]; then
  cp "$1" "$workdir/isp.deb"
  cp "$2" "$workdir/gst.deb"
else
  command -v curl >/dev/null || { echo 'Missing: curl' >&2; exit 1; }
  curl -fL --retry 2 --connect-timeout 15 --max-time 180 \
    "$base/libAWIspApi/libAWIspApi_602_1.0.0_arm64.deb" -o "$workdir/isp.deb"
  curl -fL --retry 2 --connect-timeout 15 --max-time 180 \
    "$base/gst-plugins-good1.0/debian13/gstreamer1.0-plugins-good_1.26.2-deb13u1.2_arm64.deb" -o "$workdir/gst.deb"
fi
(
  cd "$workdir"
  printf '%s  %s\n' \
    112012255a3f3e7d33faa63241dea8b6e3ab081d1ef63c56c0e656e5b5978fc9 isp.deb \
    95f54109a91ea65ee87f69856d5d00611e31caaade32196cbe8fb32b4a668fff gst.deb |
    sha256sum -c -
)
dpkg-deb -x "$workdir/isp.deb" "$workdir/isp"
dpkg-deb -x "$workdir/gst.deb" "$workdir/gst"
gcc -shared -fPIC -O2 -Wall -Wextra -Werror \
  "$source_dir/imx219-isp-compat.c" -o "$workdir/libmicroduck-imx219-compat.so" -ldl

root="$destination/opt/microduck-awisp"
bin="$destination/usr/local/bin"
src="$destination/usr/local/src"
install -d "$root/lib" "$root/gst" "$bin" "$src"
if [ -f "$bin/microduck-imx219-isp-snapshot" ]; then
  install -d "$root/backups"
  backup=$(mktemp -d "$root/backups/install.XXXXXX")
  cp -p "$bin/microduck-imx219-isp-snapshot" "$backup/"
  cp -R "$root/lib" "$root/gst" "$backup/"
  echo "Previous camera setup: $backup"
fi
for library in libAWIspApi.so libisp.so libisp_ini.so; do
  install -m 644 "$workdir/isp/usr/lib/aarch64-linux-gnu/$library" "$root/lib/$library"
done
install -m 644 "$workdir/gst/usr/lib/aarch64-linux-gnu/gstreamer-1.0/libgstvideo4linux2.so" "$root/gst/"
install -m 755 "$workdir/libmicroduck-imx219-compat.so" "$root/lib/"
install -m 644 "$source_dir/imx219-isp-compat.c" "$src/imx219_isp_compat.c"
install -m 755 "$source_dir/microduck-imx219-isp-snapshot" "$bin/"
echo "Installed camera tools under ${destination:-/}"
