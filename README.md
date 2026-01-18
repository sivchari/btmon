# btmon

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A CLI tool to monitor Bluetooth device battery levels on macOS.

## Features

- Read battery levels from Bluetooth devices using:
  - **GATT Battery Service** (UUID: 0x180F) via Core Bluetooth
  - **Private IOBluetooth APIs** for Apple devices (Magic Trackpad, AirPods, etc.)
- Filter devices by name
- JSON output support
- Works with ZMK keyboards, Magic Trackpad, AirPods, and other BLE devices

## Installation

### From Source

```bash
git clone https://github.com/sivchari/btmon-rs.git
cd btmon-rs
cargo install --path .
```

### From Releases

Download the latest release from [GitHub Releases](https://github.com/sivchari/btmon-rs/releases).

| Platform | Architecture | Download |
|----------|--------------|----------|
| macOS | Apple Silicon (arm64) | `btmon-macos-arm64.tar.gz` |
| macOS | Intel (x86_64) | `btmon-macos-x86_64.tar.gz` |

## Usage

```bash
# List all devices with battery levels
btmon

# Filter by device name
btmon -d "Adv360"

# JSON output
btmon -j

# Debug mode
btmon --debug
```

### Options

| Flag | Description |
|------|-------------|
| `-d, --device` | Filter by device name (partial match) |
| `-j, --json` | Output in JSON format |
| `--debug` | Enable debug output |
| `-h, --help` | Show help |
| `-V, --version` | Show version |

### Example Output

```bash
$ btmon
Adv360 Pro(Home): 76%
sivchari magic: 86%
```

```bash
$ btmon -j
[
  {
    "name": "Adv360 Pro(Home)",
    "address": "BLE",
    "battery_level": 76
  },
  {
    "name": "sivchari magic",
    "address": "bc-d0-74-b7-a6-b3",
    "battery_level": 86
  }
]
```

## Requirements

- macOS (uses Core Bluetooth and IOBluetooth frameworks)
- Bluetooth permission for Terminal/iTerm2

### Bluetooth Permission

If you encounter permission issues, grant Bluetooth permission:

1. Open **System Settings** > **Privacy & Security** > **Bluetooth**
2. Add your terminal app (Terminal.app, iTerm2, etc.)

## For ZMK Keyboards

Make sure your ZMK firmware has the Battery Service enabled:

```conf
CONFIG_BT_BAS=y
```

## Development

```bash
# Build
cargo build --release

# Run tests
cargo test

# Run clippy
cargo clippy

# Format code
cargo fmt
```

## License

MIT License - see [LICENSE](LICENSE) for details.
