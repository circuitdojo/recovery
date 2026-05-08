# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-05-08

### Added
- `--verify` flag: read the device lock status and exit without modifying
  anything. Exits 0 if unlocked, 1 if locked.

### Fixed
- Catch the `unreachable!()` panic in probe-rs 0.31's Glasgow driver
  (`glasgow::mux::GlasgowDevice::new_from_selector`) that fired when no
  probe was connected and no `--serial` was supplied. The probe-open
  loop now treats it as a regular `NotFound` and surfaces the standard
  timeout message instead of an unwind.

## [0.2.0] - 2026-05-08

### Added
- Quick chip erase via NVMC `ERASEALL` (per Nordic NAN_041 §9.2.1) — far
  faster than probe-rs's default sector-by-sector erase. The flash phase
  now runs with `skip_erase = true` since the chip is already empty.
- `--unlock-only` flag: run the unlock procedure (and final reset) without
  performing the NVMC erase or firmware download. A no-op when the device
  is already unlocked.
- `--erase-only` flag: unlock and erase the device without flashing.
- `--skip-uicr` flag: skip writing the APPROTECT/SECUREAPPROTECT UICR
  registers after flashing.
- `--retries` (default 3) and `--erase-timeout` (default 5000 ms) for the
  unlock/erase sequence.
- `--swd-speed` (default 1000 kHz) to control SWD clock for marginal links.
- `--force` to re-run the unlock sequence even if the device reports as
  already unlocked.
- APPROTECT/SECUREAPPROTECT UICR writes after a successful flash (skippable
  via `--skip-uicr`).
- GitHub Actions workflow that builds release binaries for x86_64 and
  aarch64 Linux, x86_64 and aarch64 macOS, and x86_64 Windows. Tags
  matching `v*` publish the binaries to a GitHub Release.

### Changed
- Migrated from `probe-rs` 0.29.1 to 0.31.0. Adapted to the renamed
  `ArmDebugInterface` trait, the new `try_into_arm_debug_interface` +
  `select_debug_port` flow, and the now-required `interface` field on
  `DebugProbeSelector`.
- The `<IMAGE>` argument is now optional; required only when neither
  `--erase-only` nor `--unlock-only` is set.
- README updated to call out the [Circuit Dojo nRF9151 Feather](https://www.circuitdojo.com/products/nrf9151-feather)
  as the target board and probe-rs as the underlying flash/debug stack.

## [0.1.0]

- Initial release.

[0.2.1]: https://github.com/circuitdojo/recovery/releases/tag/v0.2.1
[0.2.0]: https://github.com/circuitdojo/recovery/releases/tag/v0.2.0
[0.1.0]: https://github.com/circuitdojo/recovery/releases/tag/v0.1.0
