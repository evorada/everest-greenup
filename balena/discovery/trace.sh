#!/bin/sh
# Hand-driven session with a board, logged to /data/traces.
#   trace.sh /dev/ttyUSB0 [label]
# Enter sends CR (what the board expects); board lines end in CR and are shown
# one per line. Local echo is on. Leave with C-a C-x.
#
# NEVER type Z, Reset or Test: Z enters the bootloader (docs/plan.md, hazards).
set -eu

port=${1:?usage: trace.sh /dev/ttyUSBn [label]}
label=${2:-session}
mkdir -p /data/traces
log="/data/traces/$(date -u +%Y%m%dT%H%M%SZ)-${label}.log"

if [ "${GREENUP_NOHUPCL:-0}" = "1" ]; then
    stty -F "$port" -hupcl
fi

echo "port=$port log=$log"
echo "answer every 'RaspberryPi?' with 'RaspberryPiModeOK'. Never send Z / Reset / Test."
exec picocom -b 115200 --imap crcrlf --echo --noreset --logfile "$log" "$port"
