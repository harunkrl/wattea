<div align="center">

# 🔋 Wattea

**Terminal battery consumption tracker for Linux.**

*Watch your watts — see how much battery you burn per hour, live and over time.*

</div>

---

Wattea reads battery data straight from `/sys/class/power_supply/` (and UPower
history) and renders a live, detail-rich terminal dashboard: charge level, draw
rate in **%/hour**, voltage, health, cycles, a power sparkline, and — coming
soon — per-hour-of-day usage patterns.

Built to be **lightweight** (a battery tool shouldn't drain your battery): a
single static Rust binary, ~1.3 MB, minimal footprint.

## ✨ Features

**Phase 1 — Live dashboard** ✅

- Live charge **gauge** (color-coded by state/level)
- Instant **power draw** (W) and **drain rate (%/hour)**
- Voltage, estimated time to full/empty, cycle count
- **Power sparkline** of the last 5 minutes
- Battery **health** (actual vs. design capacity)

**Phase 2 — History & collection** 🚧

- Background collector daemon → SQLite time-series
- UPower `.dat` history backfill
- 24h rate & capacity trend charts

**Roadmap**

- Phase 3 — Per-hour-of-day usage patterns (the killer feature)
- Phase 4 — Session & cycle analysis, health degradation trends
- Phase 5 — CPU/brightness correlation, anomalies, CSV export

## 📦 Install

```bash
git clone <repo-url> wattea
cd wattea
cargo build --release
# binary: ./target/release/wattea
```

Arch users: an AUR package is planned.

## 🚀 Usage

```bash
wattea              # launch the live TUI dashboard
```

| Key | Action |
|-----|--------|
| `r` | refresh now |
| `q` / `Esc` | quit |

The background collector (Phase 2) runs as a systemd user service:

```bash
wattea-daemon      # samples sysfs → ~/.local/share/wattea/db.sqlite every 60s
```

## 🏗️ Architecture

```
wattea-daemon (systemd)  ──sysfs──►  SQLite  (~/.local/share/wattea/db.sqlite)
       │                                   │
       └───────────────────────────────────┘
                     ▲
                     │ reads (history) + sysfs (live)
                     │
   wattea (TUI)  ────┘   Ratatui dashboard
```

Wattea is structured as a small library + two binaries:

```
src/
├── lib.rs            # shared core: battery, storage, model
├── battery.rs        # sysfs reading → BatterySample
├── storage.rs        # SQLite time-series store
├── bin/
│   ├── wattea.rs         # TUI dashboard
│   └── wattea-daemon.rs  # background collector
```

## 🛠️ Development

```bash
cargo test                       # unit + render tests (TestBackend)
cargo clippy --all-targets       # zero warnings
cargo fmt
cargo run                        # dev build, live dashboard
```

## 🔋 Data sources

- `/sys/class/power_supply/BAT*/` — live sysfs (primary)
- UPower DBus `GetHistory` / `GetStatistics` — backfill & calibration
- `/var/lib/upower/history-*.dat` — historical backfill

Drain rate is computed as `power(W) / energy_full(Wh) × 100`.

## 📄 License

MIT — see [LICENSE](LICENSE).
