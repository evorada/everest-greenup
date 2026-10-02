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
| `docs/plan.md` | Design and plan |

Planned: `evse-session`, `legrand-driver`, `board-model`, `greenup-runtime`,
`bins/legrand-sim`, `module/RsGreenUpEvse`, Docker and Balena packaging.

## Development

Everything except the EVerest module builds and tests on macOS or Linux:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`target/` is a symlink to an external build disk on the main dev machine;
recreate it as a plain directory elsewhere if needed.

## License

MIT OR Apache-2.0, at your option. Parts are derived from
[greenup-link](https://github.com/BolzaW/greenup-link) (MIT); see `NOTICE`.
