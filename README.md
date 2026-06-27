<h1 align="center">Wattea</h1>
<p align="center"><strong>Terminal battery consumption tracker for Linux.</strong></p>
<p align="center"><em>Watch your watts — see how much battery you burn per hour, live and over time.</em></p>

---

Wattea is a terminal dashboard for tracking laptop battery consumption on Linux.
It shows live metrics — charge level, draw rate in **%/hour**, voltage, health —
plus historical trends, per-hour usage patterns, on-battery session analysis,
and correlation with CPU load, screen brightness, and temperature.

It is built to be **lightweight**: a single static Rust binary with a minimal
footprint, so the battery tool never becomes the battery drain.

## Features

**Live dashboard**

- Color-coded charge gauge, instant power draw (W) and **drain rate (%/hour)**
- Voltage, estimated time to full/empty, cycle count
- Battery **health** (actual vs. design capacity)
- Power sparkline of the last 5 minutes
- Live **CPU load, screen brightness, temperature**, and anomaly count

**History & trends**

- Background collector samples `sysfs` every 60 seconds into a local SQLite store
- Import past UPower history (`wattea import`)
- 24-hour capacity and power-draw trend chart

**Usage patterns**

- Average **%/hour by hour of day** — your usage rhythm at a glance
- Compare **by day of week**, color-coded by intensity, peak/trough highlighted

**Session analysis & export**

- Per-session table of on-battery runs (unplug → plug): duration, % drop,
  average %/h and W
- `wattea export` dumps all samples to CSV

**Anomaly detection**

- Power-draw spikes flagged by statistical deviation (z-score), surfaced live

## Install

From source (requires Rust):

```bash
git clone https://github.com/harunkrl/wattea.git wattea
cd wattea
cargo build --release
```

The binaries land in `target/release/`:

- `wattea` — the interactive TUI dashboard
- `wattea-daemon` — the background collector

To build, install both binaries, and enable the collector as a systemd user
service in one step:

```bash
./dist/install.sh
```

## Usage

Launch the dashboard:

```bash
wattea
```

It has four tabs:

| Key | Tab | What it shows |
|-----|-----|---------------|
| `1` | Live | charge gauge, %/h, sparkline, health, CPU/brightness/temp |
| `2` | Trend | 24h capacity + power chart |
| `3` | Pattern | %/h by hour-of-day / day-of-week |
| `4` | Sessions | on-battery session table |

| Key | Action |
|-----|--------|
| `Tab` | cycle tabs |
| `d` | toggle hour/day view (Pattern tab) |
| `r` | refresh now |
| `q` / `Esc` | quit |

Commands:

```bash
wattea                  # launch the dashboard
wattea import           # backfill past UPower history into the store
wattea export data.csv  # dump all samples to CSV (omit file → stdout)
```

## How it works

```
wattea-daemon (systemd)  ──sysfs──►  SQLite  (~/.local/share/wattea/db.sqlite)
                                              ▲
                                              │ reads history + live sysfs
   wattea (TUI)  ─────────────────────────────┘
```

A small background service (`wattea-daemon`) samples `/sys/class/power_supply/`
every minute and stores it locally in SQLite. The dashboard (`wattea`) reads
both that history and live sysfs. No network, no telemetry, no root required.

## License

MIT — see [LICENSE](LICENSE).
