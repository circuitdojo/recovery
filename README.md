# nRF91xx Recovery Tool

A command-line recovery tool for nRF91xx series microcontrollers, designed to unlock locked devices and flash firmware.

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

```bash
recovery <HEX_FILE>
```

Flash a hex file to the connected nRF91xx device:

```bash
recovery firmware.hex
```

### Advanced Options

```bash
recovery [OPTIONS] <HEX_FILE>

Arguments:
  <HEX_FILE>  Path to the hex file to flash

Options:
  -t, --timeout <TIMEOUT>      Timeout in milliseconds for probe connection [default: 2000]
  -f, --force                  Force unlock even if device appears unlocked
      --vendor-id <VENDOR_ID>  Vendor ID for debug probe [default: 11914]
      --product-id <PRODUCT_ID> Product ID for debug probe [default: 12]
  -s, --serial <SERIAL>        Serial number of debug probe
  -h, --help                   Print help
  -V, --version                Print version
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

## Recovery Process

The tool performs the following sequence:

1. **Probe Connection**: Connects to the debug probe with specified timeout
2. **Device Unlock**: 
   - Checks device lock status via CSW register
   - Performs CTRL-AP erase operation if locked
   - Issues soft reset for nRF91x1 devices
   - Validates unlock success
3. **Firmware Flash**: Downloads the hex file to device memory
4. **UICR Programming**: Writes protection values to UICR registers
5. **Reset**: Performs final device reset

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