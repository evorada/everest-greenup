# Discovery release

Balena release for the hardware discovery sessions (docs/plan.md, "Prepare",
step 2 and gates G1 to G9). It does not run EVerest. On the Pi it gives you:

- `trace.sh /dev/ttyUSBn [label]`: picocom session with a board, logged to
  `/data/traces/<utc-time>-<label>.log`;
- greenup-link (BolzaW, MIT; commit pinned in the Dockerfile) as the
  known-good control loop and web UI, started only when `GREENUP_PORT` is set.

Two services, `side1` and `side2`, one per board. Both share `/data`.

## Build and push

```sh
# local build (Apple silicon builds arm64 natively)
docker build --platform linux/arm64 -t greenup-discovery balena/discovery

# release to the fleet (local build, then upload)
balena deploy gaia_charge/greenup --build --source balena/discovery
# or let the Balena builders do it
balena push gaia_charge/greenup --source balena/discovery
```

## First contact (charger powered off while fitting the SD card)

1. `balena ssh <device> side1`, then `ls -l /dev/serial/by-id` (G7).
2. `trace.sh /dev/ttyUSB0 side1-first` and talk to the board by hand. Record
   `FM?`, `State?`, `CC?`, `E?`, `SoftwareVersion?` before anything else.
   Answer every `RaspberryPi?` with `RaspberryPiModeOK`.
3. Only then set the service variable `GREENUP_PORT=/dev/ttyUSB0` on `side1`
   to start greenup-link (web UI on `http://<device>:8080`). Add
   `GREENUP_FORCE_FM=0` to keep the board's own FM value; without it,
   greenup-link forces `FM:1` like upstream.
4. Copy the traces off the device with `balena ssh` + `cat`, or
   `balena device ssh`/`scp` via the local tunnel, into `tests/traces/`.

Environment (service variables):

| Variable | Default | Meaning |
| --- | --- | --- |
| `GREENUP_PORT` | unset (idle) | serial device for greenup-link |
| `GREENUP_HTTP_ADDR` | `0.0.0.0:8080` / `:8081` | greenup-link listen address |
| `GREENUP_FORCE_FM` | `1` | `0` leaves `FM` alone |
| `GREENUP_NOHUPCL` | `0` | `1` runs `stty -hupcl` before opening the port (G6) |

## Hazards

- Never send `Z`, `Reset` or `Test`. `Z` puts the board into its bootloader.
- Opening the port may reset the MCU through DTR (G6).
- `greenup-link-env.patch` is the only change to upstream greenup-link: port,
  HTTP address and FM forcing from the environment.
