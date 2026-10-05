# everest-greenup

EVerest integration for Legrand Green'Up wallboxes, starting with the
two-sided 058049 (2 × 22 kW).

The wallbox power board runs its own IEC 61851 state machine and only
accepts session-level commands: authorize, block, set the maximum current,
unlock. EVerest's usual answer, a board-support package under EvseManager,
does not fit a board like this; its BSP guide says so. So this repo has a
module of its own, `RsGreenUpEvse`, which implements the `evse_manager` and
`energy` interfaces. Auth, OCPP, the APIs and EnergyManager then run on top
of the board unchanged.

Status: early. See [docs/plan.md](docs/plan.md) for the design, the
hardware go/no-go gates and the phases.

> Not affiliated with, sponsored or endorsed by Legrand. "Green'Up" and
> "Legrand" are trademarks of their respective owners.

## Layout

| Path | What |
| --- | --- |
| `crates/legrand-proto` | Board serial protocol: commands, messages, state letters, line framing. Pure, no I/O. |
| `crates/evse-session` | Session state machine (EvseManager's event grammar), energy node, the `ChargerDriver` contract. Pure. |
| `crates/board-model` | Fake power board with one profile per open hardware question. Pure. |
| `crates/legrand-driver` | `ChargerDriver` for the Legrand board: handshake, polling, scheduling, facts, faults, energy register. Pure. |
| `crates/greenup-runtime` | Threads: a serial or TCP port per side, reader, actor loop, persistence, RX/TX trace. |
| `bins/legrand-sim` | The fake board behind TCP, with a control port to move the simulated vehicle. |
| `module/RsGreenUpEvse` | The EVerest module: everestrs adapter over the crates above. One instance per side. Builds only in the image. |
| `balena/discovery` | Balena discovery release: picocom + patched greenup-link, no EVerest. For the hardware gates. |
| `docker/everest` | EVerest (pinned upstream SHA) built for arm64 with the module baked in, plus a slim runtime image. |
| `docker/compose.sil.yaml` | Software in the loop on the Mac: EVerest, mosquitto and two simulated boards. |
| `config/` | EVerest configs installed into the image (`manager --check` runs on them at build time). |
| `docs/plan.md` | Design and plan |

Planned: the Balena release with EVerest, the differential test against
EvseManager, OCPP.

## Development

Everything except the EVerest module builds and tests on macOS or Linux:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`target/` is a symlink to an external build disk on the main dev machine;
recreate it as a plain directory elsewhere if needed.

The module type-checks on the host against the pinned EVerest checkout:

```sh
ln -s /path/to/everest-core module/everest-core
EVEREST_CORE_ROOT=$PWD/module/everest-core cargo check --manifest-path module/Cargo.toml
```

Images (arm64, built natively on Apple silicon):

```sh
docker build --platform linux/arm64 -t greenup-discovery balena/discovery
docker build --platform linux/arm64 -f docker/everest/Dockerfile -t everest-greenup .
```

Software in the loop on the Mac, two simulated boards under EVerest:

```sh
docker compose -f docker/compose.sil.yaml up --build
echo plug | nc localhost 5102        # side 1 vehicle plugs in; 5202 is side 2
```

See the header of `docker/compose.sil.yaml` for authorizing and watching
events.

## License

MIT, see `LICENSE`. Parts are derived from
[greenup-link](https://github.com/BolzaW/greenup-link) (MIT); see `NOTICE`.
