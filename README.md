# nRF91xx Recovery Tool

A command-line recovery tool for nRF91xx series microcontrollers, designed to unlock locked devices and flash firmware.

It targets the [Circuit Dojo nRF9151 Feather](https://www.circuitdojo.com/products/nrf9151-feather) and is built on top of [`probe-rs`](https://probe.rs/) for debug-probe communication and flashing.

## Overview

This tool provides recovery functionality for nRF91xx devices by:
- Unlocking locked/protected devices through CTRL-AP erase operations
- Flashing hex firmware files
- Writing UICR (User Information Configuration Registers) values
- Performing device reset operations

## Prerequisites

- Rust toolchain (install from https://rustup.rs/)
- Compatible debug probe (default: Raspberry Pi Pico with picoprobe firmware)
- nRF91xx target device

## Installation

```bash
cargo install --path .
```

## Usage

### Basic Usage

Flash a hex file to the connected nRF91xx device:

```bash
recovery firmware.hex
```

### Advanced Options

```bash
recovery [OPTIONS] [IMAGE]

Arguments:
  [IMAGE]  Path to the hex file to flash (optional if --erase-only or --unlock-only is used)

Options:
  -t, --timeout <TIMEOUT>            Timeout in milliseconds for probe connection [default: 2000]
  -f, --force                        Force unlock even if device appears unlocked
      --vendor-id <VENDOR_ID>        Vendor ID for debug probe [default: 0x2e8a]
      --product-id <PRODUCT_ID>      Product ID for debug probe [default: 0x000c]
  -s, --serial <SERIAL>              Serial number of debug probe
  -r, --retries <RETRIES>            Number of retry attempts for unlock/erase operations [default: 3]
      --erase-timeout <MS>           CTRL-AP erase timeout in milliseconds [default: 5000]
      --skip-uicr                    Skip APPROTECT/SECUREAPPROTECT UICR write after flashing
      --erase-only                   Unlock + erase the device, skip flashing
      --unlock-only                  Only unlock if locked; skip erase and flashing
      --verify                       Read lock status and exit (0 = unlocked, 1 = locked)
      --swd-speed <KHZ>              SWD speed in kHz (lower is more reliable) [default: 1000]
      --no-reset                     Skip the final device reset
  -h, --help                         Print help
  -V, --version                      Print version
```

### Examples

Force unlock a device:
```bash
recovery --force firmware.hex
```

Use a specific debug probe by serial number:
```bash
recovery --serial ABC123 firmware.hex
```

Set custom timeout for probe connection:
```bash
recovery --timeout 5000 firmware.hex
```

Use different probe vendor/product IDs:
```bash
recovery --vendor-id 0x1366 --product-id 0x1051 firmware.hex
```

Erase the device without flashing new firmware:
```bash
recovery --erase-only
```

Unlock a locked device without erasing or flashing (note: unlocking a locked nRF91 inherently erases flash via CTRL-AP, so this is a no-op when the device is already unlocked):
```bash
recovery --unlock-only
```

Flash without writing the APPROTECT/SECUREAPPROTECT UICR registers:
```bash
recovery --skip-uicr firmware.hex
```

Check whether the device is currently unlocked, without modifying anything (exits 0 if unlocked, 1 if locked):
```bash
recovery --verify
```

## Recovery Process

The tool performs the following sequence:

1. **Probe Connection**: Connects to the debug probe with specified timeout
2. **Device Unlock** (skipped when already unlocked unless `--force`):
   - Checks device lock status via CSW register
   - Performs CTRL-AP `ERASEALL` if locked (this also erases RAM and UICR)
   - Issues soft reset for nRF91x1 devices
   - Retries up to `--retries` times before giving up
3. **Quick Chip Erase**: Issues an NVMC `ERASEALL` per Nordic's NAN_041 §9.2.1.
   This is much faster than probe-rs's default sector-by-sector erase
   (roughly the time of two page erases vs. all 256 pages). Skipped in
   `--unlock-only` mode.
4. **Firmware Flash**: Downloads the hex file to device memory with
   probe-rs's sector erase phase disabled (the chip is already erased).
   Skipped in `--erase-only` and `--unlock-only` modes.
5. **UICR Programming**: Writes `0x50FA50FA` to the APPROTECT and
   SECUREAPPROTECT UICR registers (skipped with `--skip-uicr` or in
   erase/unlock-only modes).
6. **Reset**: Performs final device reset (skipped with `--no-reset`)

## Supported Devices

- nRF9151_xxAA (primary target)
- Other nRF91xx series devices (with potential minor modifications)

## Debug Probe Support

Default configuration targets Raspberry Pi Pico with picoprobe firmware:
- Vendor ID: 0x2e8a
- Product ID: 0x000c

Other probe types can be specified using `--vendor-id` and `--product-id` options.

## Error Handling

The tool provides detailed error messages for common failure scenarios:
- File not found errors for missing hex files
- Probe connection timeouts
- Device unlock failures
- Flashing errors
- UICR write failures

## Logging

Enable debug logging by setting the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug recovery firmware.hex
```

## UICR Values

The tool writes the following fixed UICR values:
- Address 0x00FF8000: 0x50FA50FA
- Address 0x00FF802C: 0x50FA50FA

These values are specific to the nRF91xx recovery process.

## Dependencies

- `probe-rs`: Debug probe communication and flashing
- `clap`: Command-line argument parsing  
- `chrono`: Timestamp handling
- `thiserror`: Error type definitions
- `env_logger`: Logging infrastructure

## License

Apache-2.0