# Plan: our own EVerest EVSE module (replaces EvseManager) for the Legrand Green'Up 058049

## Context

- `greenup-link` (BolzaW, MIT) replaces Legrand's Pi software. It talks to the wallbox power board (ATmega2560, Legrand firmware, unchanged) over USB serial at 115200 baud, using ASCII lines ending in `\r`. It supports only the 058001, in direct-charge mode.
- We own a 058049: 2 sides, 2×22 kW (32 A, 3-phase), and each side has a Type 2 socket plus a Mode 2 domestic socket. Each side has its own controller on its own USB serial port. The comm-kit SD card is dead, so the Pi 3B gets a new OS.
- Goal: put both sides under EVerest, first for **load sharing** between them, then **OCPP** (via `evorada/maeve-csms`).
- Decisions made:
  - native `everestrs` module compiled into a custom EVerest build;
  - a new repo in the `evorada` org;
  - a Balena release (`raspberrypi3-64`) as the artifact.
- Related: issue on `FM` https://github.com/BolzaW/greenup-link/issues/9. Local EVerest checkout for reference: `/Users/suda/Projects/ModDefOrg/EVerest` (fd2a058e).

## Why not a BSP under EvseManager

The board runs its own IEC 61851 state machine. Its commands work at session level: authorize, block, set max current, unlock. EvseManager expects a BSP that hands it control-pilot-level control (PWM duty, X1/F, relay permission). EVerest's BSP guide states that chargers like this "can't be supported" because two state machines would conflict. So we implement the layer the board actually matches: the `evse_manager` and `energy` interfaces, the same layer EvseManager provides.

| | BSP translator (rejected) | Own `evse_manager` + `energy` (chosen) |
| --- | --- | --- |
| Fit with the board | emulates CP events we can't observe (B1/B2, PWM state) | 1:1 with `T2COK`/`T2CNOK`/`CC:NN`/`Unlock` |
| EVerest guidance | explicitly unsupported | not covered; precedents exist only as a stub (`modules/Simulation/EvseManagerSimulator`) and a proxy (`mhei/remotechargeport`) |
| Session hang risks | EvseManager waits forever for PowerOff and for PP; events dropped while disabled; 30 s wake-up churn | our own logic, no hidden waits |
| Hardware gates that block | G1, G2, G3, G4, PP | G1, G4 (G2 only affects pause/resume) |
| OCPP/Auth correctness | free (EvseManager) | ours to get right; checked against EvseManager by a differential test in SIL |
| Load sharing | free | small energy component (`enforce_limits` → `CC:NN`) |
| Breaking changes to providers since 2025 | 3 | about 10 (5 breaking changes, 3 added commands, pending `Connector` fields) |
| Debugging | two black-box state machines interacting, on hardware | open-source consumers, reproducible in SIL |
| Reach | Legrand only | the session core is driver-agnostic and suits any wallbox that runs its own charging logic (EVerest has no module for these today) |

The two costs we accept: the interface churns more (pin the version, upgrade deliberately, run a weekly canary build), and OCPP/Auth fidelity depends on our code (differential tests, property tests, MaEVe).

## Board hazards

- `Z\r` puts the board into its bootloader (upstream `src/flash.rs:521-531`). `Reset` and `Test` are dangerous too. The `Command` enum has no variant for any of them and there is no raw passthrough.
- Opening the port might reset the MCU through DTR (it is flashed with `avrdude -cwiring`). Gate G6 checks it; `stty -hupcl` if needed.
- Always answer `RaspberryPi?` with `RaspberryPiModeOK`. What the board does when the answers stop is gate G5.
- `FM` is forced to `1` (greenup-link `src/serial.rs:161-166`) and is undocumented. Record the native value first.

## Architecture

```
Balena release (raspberrypi3-64), network_mode: host
  mosquitto (127.0.0.1:1883 only)
  everest = EVerest @ pin + RsGreenUpEvse built in-tree
     Auth ──────────┐
     OCPP201 ───────┼──► evse_1.evse / evse_2.evse              (evse_manager)
     API ───────────┘
     EnergyNode(fuse) ─► evse_1.energy_grid / evse_2.energy_grid (energy)
     EnergyManager ── energy_trunk ─► EnergyNode
  evse_1, evse_2: one RsGreenUpEvse instance per side, each on its own port
  (config `port`, `side`). The driver checks the board's `Side?` answer
  against `side` and raises Inoperative on a mismatch, so a swapped USB
  enumeration is caught instead of silently crossing the sides. No
  EvseManager, no BSP.
```

Built on 2026-10-04: every crate in the table below, the module, the image
build and the SIL config. Still open: the hardware gates, the differential
test, OCPP.

### Repo `evorada/everest-greenup`
```
crates/legrand-proto     pure: Command (safe variants only), Message::parse (exact keys),
                         StateCode, LineFramer (byte-based, lossy per line, 256 B cap)
crates/evse-session      pure: per-connector session state machine, energy component,
                         error set; EVerest-agnostic mirror types; trait ChargerDriver
crates/legrand-driver    pure: ChargerDriver for the Legrand board (command scheduler,
                         sync/poll, state→fact mapping, energy register)
crates/board-model       pure fake board with one profile per Phase 1 unknown
crates/greenup-runtime   std threads: serial or TCP port per side, reader, actor loop,
                         persistence, RX/TX trace
bins/legrand-sim         board-model behind TCP (board port + control port); socat makes
                         a pty out of it when a program insists on a serial device
module/RsGreenUpEvse     everestrs adapter (manifest, build.rs, main.rs, convert.rs)
docker/  balena/  tests/traces/  tests/differential/
```
- Every crate except the module builds and tests on macOS with plain cargo. The module builds only in the Linux/Docker build.
- greenup-link code is copied with its MIT notice: `src/serial.rs:10-235` (loop, handshake, ping, parser, FM, energy), `src/index.html:491-573` (letters), `src/api.rs` (command strings). License the new crates MIT.

### everestrs facts
- Bindings: `lib/everest/framework/everestrs` v0.26 (cxx, not on crates.io). Flag `-DEVEREST_ENABLE_RS_SUPPORT=ON`; the module directory must start with `Rs`; add `ev_add_module(...)`. Rust ≥ 1.86.
- `build.rs`: `Builder::new("manifest.yaml", vec![EVEREST_CORE_ROOT]).generate()`, then `include!(…/generated.rs)`.
- Generated traits: `EvseManagerServiceSubscriber` and `EnergyServiceSubscriber`. Handlers are sync and run on a C++ pool; a panic aborts. No shutdown hook, so handle SIGTERM ourselves. Use a TCP broker.
- The `max_connections != 1` limit applies only to *requirements*. We require nothing, so OCPP, Auth and the API can all connect to `evse_n`.
- The CMake Rust path isn't CI-tested upstream, so spike it in step 3. Build natively on arm64 (M1, GH arm64 runner).
- Reference modules: `modules/HardwareDrivers/PowerMeters/RsIskraMeter` (Rust structure, mockall) and `modules/Simulation/EvseManagerSimulator` (the startup handshake for this interface, 124 lines).

### Contract the module must honour
Sources: consumers at fd2a058e. OCPP: `modules/EVSE/OCPP201`, `OCPP`, `OCPPmulti`, `lib/everest/ocpp`. Auth: `modules/EVSE/Auth/lib/AuthHandler.cpp`. Event order: `modules/EVSE/EvseManager/docs/index.rst:180-231` and `Charger.cpp` emitters.

#### Startup

1. Publish `hw_capabilities` (phases 3, 10-32 A, `IEC62196Type2Socket`), `supported_energy_transfer_modes` (`AC_three_phase_core`), `powermeter`, `limits`, `selected_protocol: "IEC61851-1"`.
2. Publish `waiting_for_external_ready: true`. On `external_ready_to_start_charging`, publish `ready: true` and emit `Enabled` (with `source`).
3. Without OCPP, publish `ready: true` right away (config flag), because the legacy API waits for it.

#### get_evse

Answer `{id: n, connectors: [{id: 1, type: sType2}]}`. The EVSE id must equal the connection index + 1 in Auth/OCPP and the module `mapping`.

#### Session events

The order is SessionStarted(EVConnected or Authorized) → AuthRequired → Authorized → TransactionStarted → PrepareCharging → ChargingStarted ⇄ ChargingPausedEV / ChargingPausedEVSE → StoppingCharging → ChargingFinished → TransactionFinished(reason) → SessionFinished, the last one only on unplug.

Every event carries its context object (`session_started`, `transaction_started`, … each with a `meter_value`). The OCPP 1.6 module calls `.value()` on these and crashes EVerest if one is missing.

The `uuid` is at most 36 ASCII characters and stable for the session; it becomes the OCPP 2.x transactionId.

#### authorize_response

- Only `Accepted` counts. Echo the `ProvidedIdToken` unchanged as `id_tag`, and copy `reservation_id`.
- Emit TransactionStarted within Auth's `connection_timeout` (OCPP default 120 s).
- Authorization that arrives before plug-in is held, but `T2COK` is never sent in board state A.

#### Other commands

| Command | Session effect | Board effect |
| --- | --- | --- |
| `withdraw_authorization` | drop a pending authorization; Deauthorized (+ SessionFinished / PluginTimeout, as EvseManager does) | none |
| `stop_transaction(req)` | StoppingCharging … TransactionFinished(req.reason, req.id_tag); true only during a transaction | `T2CNOK` |
| `pause_charging` / `resume_charging` | ChargingPausedEVSE(UserPause) / resume; true only during a transaction | `T2CNOK` / `T2COK` (resume needs G2) |
| `enable_disable(c, src)` | priority arbitration like EvseManager; return "enabled after"; emit Enabled/Disabled only on change | `T2CNOK` on disable |
| `reserve(id)` / `cancel_reservation` | false unless idle and enabled and not faulted; ReservationStart/End; a used reservation puts `reservation_id` into TransactionStarted | none |
| `force_unlock` | stop the transaction (UnlockCommand), then true | `Unlock` when not delivering |
| `set_plug_and_charge_configuration` | no-op | none |
| `update_allowed_energy_transfer_modes`, `set_der_available` | `NoHlc` | none |

#### Errors

Raise `evse_manager/Inoperative` (message carries the Legrand detail) for:

- board fault letters;
- `E:xxxx` ≠ 0000;
- link down for more than 3 s;
- FM that won't stick;
- unauthorized charging;
- `T2CNOK` ignored for 10 s.

Clear it when the condition is gone. Auth and OCPP derive "Faulted" from this error. Map to `MREC4`/`MREC5` later, once the E codes are known.

#### powermeter

- Publish about every 1 s while charging and every 10 s when idle. Fields: `energy_Wh_import.total` (must never decrease; persisted), `power_W.total`, `voltage_V.L1`, `frequency_Hz.L1`. It feeds OCPP MeterValues.
- Hold back L1-L3 currents until G9 shows what they mean.

#### Energy component (energy_n)

- Publish `energy_flow_request` every 1 s: `uuid` = impl id, `node_type: Evse`, `children: []`, one `schedule_import` entry "now" with `limits_to_root.ac_max_current_A` 32, `ac_min_current_A` 10 (G4), phase counts 3/3, `ac_number_of_active_phases` 3, and `limits_to_leaves` 32. Export entry 0. Empty setpoints.
- `evse_state` follows EvseManager's mapping. Set `priority_request: true` at startup and on WaitForAuth/Finished.
- `enforce_limits`: ignore other uuids. Take the per-phase limit (convert W via 230 V × phases). Fall to 0 A when `valid_for` expires. Below min: `T2CNOK` + ChargingPausedEVSE(NoEnergy). Otherwise `CC:floor(limit)`, rate-limited (at least 1 A change, at most one per 5 s), then resume. Publish `limits` and `enforced_limits`.
- EnergyManager is therefore required in every config: it holds 0 A until the first limit arrives.

### Board side (`legrand-driver`)
- The driver interface is `ChargerDriver { authorize(bool), set_current(A), unlock(), facts() }`.
- Facts from the board's state letters:

| Letters | Fact |
| --- | --- |
| A, L, Y | unplugged (Y → send `SNOK`, never `SOK`) |
| B | plugged, not authorized |
| C | authorized, EV not drawing |
| D, E | delivering |
| W | stopping |
| M | session ended by board |
| F G H R S T U V | fault |

  `FCS:` and `WT:…` reports go to the session as hints. The session core decides the events.
- On (re)connect the driver sends `RaspberryPiModeOK`, `BTNOK`, `2PCNOK` (domestic socket disabled, configurable), `Side?`, `FM?` (then `FM:1`), `State?`, `CC?`, `E?`. After that it polls `State?` every 5 s and `E?`/`CC?`/`FM?`/`BT?` every 60 s.
- The scheduler is state-based (wanted / last sent / reported): at most one command per 150 ms, ping reply first, `CC` before `T2COK`.
- Unauthorized charging (the board delivers while the session didn't authorize) gets `T2CNOK` every 2 s plus Inoperative.
- When the board is already mid-charge at startup, send `T2CNOK` and start a fresh session (re-authorization needed). Any transaction we persisted gets closed the way EvseManager's `cleanup_transactions_on_startup` (`Charger.cpp:1577`) does it.
- On SIGTERM, send `T2CNOK` on all ports, persist the energy counters and exit.

## Phases

### Prepare
1. Desk, 1-2 h.
   - Ask BolzaW (issue #9) for the Java state and error tables and the meaning of FM.
   - Get a Type 2 EV simulator (CP B/C/D switch, diode fault).
   - Pin upstream `main` at a fixed SHA and move to `2026.10.0` once it's tagged. `2026.02.1` predates many evse_manager changes.
2. Hardware discovery on Balena, 2-3 sessions (8-12 h).
   - Flash balenaOS `raspberrypi3-64`. Fit the SD card with the charger **powered off**.
   - Push a `discovery` release: an Ubuntu container with picocom and greenup-link for `aarch64-unknown-linux-gnu`. greenup-link needs a 2-line local patch so the port and HTTP port come from env (`src/serial.rs:11`, `src/main.rs:41`).
   - Outputs: `tests/traces/*.log`, `docs/board-behaviour.md`.
3. Build spike, half a day to a day, in parallel with 2. Docker (ubuntu:24.04 arm64) builds EVerest at the pin with Rust enabled, `-DEVEREST_INCLUDE_MODULES` from `applications/utils/scripts/config2cmake.py`, and `RsExample` in-tree. It must pass on the M1 and in GH Actions arm64, with `manager --check` green. If the Rust CMake path is broken, patch it or switch to Bazel.

| Gate | Experiment | Go | If it fails |
| --- | --- | --- | --- |
| G1 | `FM:1` + `BTNOK` + `2PCNOK`; plug in; 5 min with no `T2COK` | stays B, no PWM | **No-go** for auth/OCPP: only metering + static `CC` are possible |
| G2 | `T2CNOK` mid-charge, then `T2COK` while still plugged | stops ≤ 3 s, resumes | `resume_charging` returns false; an energy pause becomes stop + replug |
| G4 | `CC` 6..32 idle and mid-charge; power-cycle, then `CC?` | applied ≤ 5 s; minimum known | no load sharing (static CC); if CC lands in EEPROM, slow updates down |
| G5 | stop answering pings (idle, C, charging) | board stops; time recorded | document it; the SIGTERM `T2CNOK` covers clean shutdowns |
| G6 | open/close the port mid-charge; reboot the Pi | no reset | `stty -hupcl` |
| G7 | `/dev/serial/by-id`, `Side?` per port | `Side:1/2` | bind by `by-id` |
| G8 | D vs E vs W/M/L; car pausing; car full (`FCS`); domestic socket load | letters map clearly to facts | adjust the mapping; domestic socket stays off |
| G9 | Volt/CCI/Freq/Ener rates and units (clamp meter); ping interval; `SoftwareVersion?` | (no gate, just facts) | meter design, firmware allowlist |

### Build
4. Pure crates, 5-7 days: `legrand-proto`, `evse-session`, `legrand-driver`, `board-model`, with the tests listed under Verification.
5. Runtime, module and SIL, 3-4 days.
   - `greenup-runtime`, `RsGreenUpEvse`, `legrand-sim`.
   - `config/config-greenup-sil.yaml`: Auth + DummyTokenProvider ×2 + DummyTokenValidator (`PlugEvents`), EnergyManager + EnergyNode (`fuse_limit_A`, `phase_count: 3`), API.
   - Docker compose for the Mac (mosquitto + everest + sim). Measure RAM with `docker stats` as a stand-in for the Pi 3.
6. Differential test, 1-2 days.
   - Run the same scenarios on upstream SIL (EvseManager + YetiSimulator + EvManager) and on ours (RsGreenUpEvse + legrand-sim): plug-auth-charge-unplug, RemoteStart before plug, stop via API, pause/resume, limit below min, disable/enable, reservation.
   - Record `session_event` over MQTT, normalize (types, order, context objects, reasons) and compare. Any difference is a bug or a documented exception.

### Deploy
7. The 058049, side 1, 2-3 days.
   - Balena compose (`mosquitto` + `everest` image from GHCR), `network_mode: host`, `privileged: true` for serial (tighten later), volume `/data`, config rendered from device variables.
   - EV simulator first, then a car.
   - Memory gate: if a session leaves less than about 200 MB free on the Pi 3, trim modules, or move to a Pi 4 (check its power supply and that it fits in the kit).
8. Both sides and load sharing, 1-2 days. The fuse limit comes from an env var. Check the split and the pause below 10 A.
9. OCPP against `evorada/maeve-csms`, 1-2 days.
   - Add `OCPP201` + `EvseSecurity`; wire Auth's provider/validator to OCPP and `evse_energy_sink` to the EnergyNodes (as upstream `config-sil-ocpp.yaml`).
   - CSMS URL and security profile come from env vars.
   - Move to `OCPPmulti` once it's no longer experimental.
10. Soak, 1-2 weeks passive, and upkeep. Daily sessions with traces kept; rollback is a greenup-link Balena release. A weekly CI canary builds against EVerest `main` to catch interface breaks early.

### Later
- RFID token provider.
- Linky headroom via `external_energy_limits_consumer_API`.
- Domestic socket.
- More `ChargerDriver`s (other wallboxes).
- Offer the protocol crate to greenup-link.

## Verification

1. Tests on macOS: `cargo test --workspace --exclude RsGreenUpEvse` passes, with:
   - trace replay: every line in `tests/traces/` parses;
   - table-driven session scenarios against every board-model profile;
   - property tests on the event grammar: sequences match the allowed order; every event has its context object; the uuid is stable and at most 36 characters; Enabled/Disabled appear only on change; SessionFinished only on unplug; no `T2COK` in A or without an accepted authorization; `CC` ≤ the enforced limit; 0 A after `valid_for` expires; Inoperative always clears once its cause is gone;
   - framer fuzzing.
2. Build: the Docker build passes on the M1 and in GH Actions arm64; the image contains `libexec/everest/modules/RsGreenUpEvse/`; `manager --check` accepts the SIL and Balena configs.
3. Differential test (step 6): no unexplained differences from EvseManager's event streams.
4. SIL with OCPP, MaEVe running locally:
   - Boot;
   - StatusNotification Available → Occupied → Available;
   - TransactionEvent Started/Updated/Ended with meter values and the right stoppedReason;
   - RequestStart/StopTransaction, ChangeAvailability, UnlockConnector;
   - SetChargingProfile capping `CC`.
5. HIL: EV simulator, then a car on side 1. Then both cars with a 32 A fuse: each side gets `CC:16`, and after one unplugs the other goes to `CC:32`.
6. Failure drills:
   - kill the manager mid-charge (`T2CNOK`; board behaves as G5 recorded);
   - unplug one board's USB (Inoperative on that EVSE only);
   - power-cycle the Pi (no board reset, no stuck transaction);
   - Balena update mid-charge;
   - Pi 3 free RAM stays above ~200 MB.
