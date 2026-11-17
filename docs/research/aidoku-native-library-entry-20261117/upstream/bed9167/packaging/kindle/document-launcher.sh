#!/bin/sh
# Name: Aidoku
# DontUseFBInk
# Author: Aidoku
# Icon: /mnt/us/aidoku/assets/aidoku-cover-v2.png
set -eu
umask 077
LOG_DIR=/mnt/us/aidoku/data/logs
mkdir -p "$LOG_DIR"
exec </dev/null >>"$LOG_DIR/document-entry.log" 2>&1
exec sh /mnt/us/aidoku/launch.sh --library
