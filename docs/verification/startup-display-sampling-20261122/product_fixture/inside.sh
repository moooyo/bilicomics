#!/bin/bash
set -euo pipefail
umask 077
cd /work
test "$#" -eq 2 && test "$(cat /proc/1/comm)" = ubuntu-run
python3 -IB -c 'from socket import if_nameindex; raise SystemExit(if_nameindex() != [(1, "lo")])'
for target in /mnt /usr/sbin /dev /run /proc/bus/input/devices; do
    test "$(findmnt -n -o PROPAGATION -T "$target")" = private
done
test "$(stat -c '%u:%a' /work)" = 0:700
test "$(findmnt -n -o FSTYPE -T /dev)" = tmpfs
test "$(findmnt -n -o FSTYPE -T /run)" = tmpfs
test "$(findmnt -n -o FSTYPE -T /proc/self)" = proc
test -d /proc/bus/input && test -f /proc/bus/input/devices
test ! -e /dev/input && test ! -e /dev/fb0 && test ! -e /mnt/us
test ! -e /usr/bin/lipc-get-prop && test ! -e /bin/lipc-get-prop
mount -t tmpfs -o size=400m,mode=0700,nosuid,nodev tmpfs /mnt
mount -t tmpfs -o size=1m,mode=0755,nosuid,nodev tmpfs /usr/sbin
mkdir -m 700 /mnt/us /run/display-product
install -m 700 /work/inputs/getter.py /usr/sbin/lipc-get-prop
printf 'B: ABS=0\n' >/run/display-product/input-inventory
mount --bind /run/display-product/input-inventory /proc/bus/input/devices
(set -C; : >/dev/fb0)
test "$(stat -c %F /dev/fb0)" = 'regular empty file'
test "$(stat -c '%d:%i' /proc/bus/input/devices)" = "$(stat -c '%d:%i' /run/display-product/input-inventory)"
cat >/mnt/us/fixture-binary
chmod 700 /mnt/us/fixture-binary
test "$(stat -c %s /mnt/us/fixture-binary)" = "$1"
exec python3 -IB /work/inputs/product_run.py "$1" "$2"
