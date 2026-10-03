#!/bin/sh
# Service entry point. Idle until GREENUP_PORT is set (a Balena service
# variable), then run greenup-link on that port. Idle first on purpose: the
# first contact with a board is done by hand with trace.sh, so the native FM
# value gets recorded before greenup-link forces FM:1 (docs/plan.md, hazards).
set -eu

mkdir -p /data/traces /data/greenup-link
cd /data/greenup-link

echo "serial devices:"
ls -l /dev/serial/by-id 2>/dev/null || echo "  (none; udev not ready or nothing plugged in)"
ls /dev/ttyUSB* /dev/ttyACM* 2>/dev/null || true

if [ -z "${GREENUP_PORT:-}" ]; then
    echo "GREENUP_PORT is unset: idling. Open a shell on this service and run:"
    echo "  trace.sh /dev/ttyUSB0 <label>      # hand session, logged to /data/traces"
    echo "Set GREENUP_PORT (and GREENUP_FORCE_FM=0 to keep the board's FM) to start greenup-link."
    exec sleep infinity
fi

if [ "${GREENUP_NOHUPCL:-0}" = "1" ]; then
    # Keep DTR asserted across close/open so the MCU is not reset (gate G6).
    stty -F "$GREENUP_PORT" -hupcl
fi

echo "starting greenup-link: port=$GREENUP_PORT http=${GREENUP_HTTP_ADDR:-0.0.0.0:8080} force_fm=${GREENUP_FORCE_FM:-1}"
exec greenup-link
