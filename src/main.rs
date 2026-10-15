use chrono::Utc;
use clap::Parser;
use probe_rs::{
    architecture::arm::{
        ap::{ApRegister, CSW, IDR},
        dp::DpAddress,
        sequences::DefaultArmSequence,
        ArmDebugInterface, FullyQualifiedApAddress,
    },
    flashing::{self, Format},
    probe::{list::Lister, DebugProbeError, DebugProbeSelector, Probe, ProbeCreationError},
    MemoryInterface, Permissions, Session,
};

use std::thread;
use std::{path::PathBuf, time::Duration};
use thiserror::Error;

#[derive(Parser)]
#[command(name = "recovery")]
#[command(about = "nRF91xx recovery tool")]
#[command(version)]
struct Args {
    #[arg(help = "Path to the hex file to flash (optional if --erase-only is used)")]
    image: Option<PathBuf>,

    #[arg(
        short,
        long,
        default_value_t = 2000,
        help = "Timeout in milliseconds for probe connection"
    )]
    timeout: u64,

    #[arg(short, long, help = "Force unlock even if device appears unlocked")]
    force: bool,

    #[arg(long, help = "Vendor ID for debug probe", default_value_t = 0x2e8a)]
    vendor_id: u16,

    #[arg(long, help = "Product ID for debug probe", default_value_t = 0x000c)]
    product_id: u16,

    #[arg(short, long, help = "Serial number of debug probe")]
    serial: Option<String>,

    #[arg(
        short,
        long,
        default_value_t = 3,
        help = "Number of retry attempts for unlock/erase operations"
    )]
    retries: u32,

    #[arg(long, default_value_t = 5000, help = "Erase timeout in milliseconds")]
    erase_timeout: u64,

    #[arg(long, help = "Skip UICR write after flashing")]
    skip_uicr: bool,

    #[arg(long, help = "Only perform erase operation, skip flashing")]
    erase_only: bool,

    #[arg(
        long,
        help = "Only unlock the device if locked; skip erase and flashing. Note: unlocking a locked nRF91 inherently erases flash via CTRL-AP."
    )]
    unlock_only: bool,

    #[arg(
        long,
        help = "Read the device lock status and exit without modifying the device. Exits 0 if unlocked, 1 if locked."
    )]
    verify: bool,

    #[arg(
        long,
        default_value_t = 1000,
        help = "SWD speed in kHz (lower is more reliable)"
    )]
    swd_speed: u32,

    #[arg(
        long,
        help = "Skip the final device reset, for callers that reset the device themselves"
    )]
    no_reset: bool,
}

#[derive(Error, Debug)]
pub enum RecoveryError {
    #[error("Programming error {0}")]
    ProbeError(#[from] probe_rs::Error),
    #[error("File download error {0}")]
    FlashingError(#[from] probe_rs::flashing::FileDownloadError),
    #[error("Imei error")]
    ImeiError,
    #[error("Timeout error")]
    TimeoutError,
    #[error("Arm interface error {0}")]
    ArmError(#[from] probe_rs::architecture::arm::ArmError),
    #[error("Debug probe error {0}")]
    DebugProbeError(#[from] probe_rs::probe::DebugProbeError),
    #[error("{0}")]
    UnlockError(String),
    #[error("UICR write needs mass erase")]
    UicrWriteNeedsMassErase,
    #[error("File not found: {0}")]
    FileNotFound(String),
}

fn rapid_chip_erase(
    iface: &mut dyn ArmDebugInterface,
    timeout_ms: u64,
) -> Result<(), RecoveryError> {
    const CTRL_AP: FullyQualifiedApAddress = FullyQualifiedApAddress::v1_with_default_dp(4);
    const ERASEALL: u64 = 0x004;
    const ERASEALLSTATUS: u64 = 0x008;

    // Start ERASEALL operation
    iface.write_raw_ap_register(&CTRL_AP, ERASEALL, 1)?;
    log::info!("Started ERASEALL operation");

    // Poll for completion with optimized timing
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    // Initial fast polling for 100ms (most erases complete quickly)
    while start.elapsed() < Duration::from_millis(100) {
        let status = iface.read_raw_ap_register(&CTRL_AP, ERASEALLSTATUS)?;
        if status == 0 {
            log::info!("Erase completed in {:?}", start.elapsed());
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }

    // Slower polling after initial period
    while start.elapsed() < timeout {
        let status = iface.read_raw_ap_register(&CTRL_AP, ERASEALLSTATUS)?;
        if status == 0 {
            log::info!("Erase completed in {:?}", start.elapsed());
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err(RecoveryError::TimeoutError)
}

fn perform_device_reset(iface: &mut dyn ArmDebugInterface) -> Result<(), RecoveryError> {
    const CTRL_AP: FullyQualifiedApAddress = FullyQualifiedApAddress::v1_with_default_dp(4);
    const RESET: u64 = 0x000;

    // Perform soft reset via CTRL-AP (for nRF91x1)
    iface.write_raw_ap_register(&CTRL_AP, RESET, 1)?;
    thread::sleep(Duration::from_millis(1));
    iface.write_raw_ap_register(&CTRL_AP, RESET, 0)?;
    thread::sleep(Duration::from_millis(20));
    log::info!("Performed soft reset");

    Ok(())
}

fn check_debug_access(iface: &mut dyn ArmDebugInterface) -> Result<bool, RecoveryError> {
    const APP_MEM: FullyQualifiedApAddress = FullyQualifiedApAddress::v1_with_default_dp(0);

    let csw = iface.read_raw_ap_register(&APP_MEM, CSW::ADDRESS)?;
    let dbg_status = (csw >> 6) & 1;
    log::info!("CSW: 0x{:08x}, DbgStatus: {}", csw, dbg_status);

    Ok(dbg_status == 1)
}

fn halt_cpu_if_possible(iface: &mut dyn ArmDebugInterface) -> Result<(), RecoveryError> {
    const APP_MEM: FullyQualifiedApAddress = FullyQualifiedApAddress::v1_with_default_dp(0);
    const DHCSR: u64 = 0xE000EDF0; // Debug Halting Control and Status Register
    const C_HALT: u32 = 0x2;
    const C_DEBUGEN: u32 = 0x1;
    const DBGKEY: u32 = 0xA05F0000;

    // Try to halt the CPU to stop any running application
    match iface.write_raw_ap_register(&APP_MEM, DHCSR, DBGKEY | C_DEBUGEN | C_HALT) {
        Ok(_) => {
            log::info!("CPU halted successfully");
            thread::sleep(Duration::from_millis(10));
        }
        Err(e) => {
            log::warn!("Could not halt CPU (device may be locked): {:?}", e);
            // Continue anyway - this is expected if device is locked
        }
    }

    Ok(())
}

/// Reads the CSW.DbgStatus bit on AP0 to determine whether the application
/// access port is enabled. Does not modify any device state.
fn verify_unlock_status(mut probe: Probe, args: &Args) -> Result<bool, RecoveryError> {
    let _ = probe.set_speed(args.swd_speed);
    probe.attach_to_unspecified()?;

    let mut iface = probe
        .try_into_arm_debug_interface(DefaultArmSequence::create())
        .map_err(|(_p, e)| RecoveryError::ArmError(e))?;

    iface.select_debug_port(DpAddress::Default)?;

    check_debug_access(iface.as_mut())
}

fn try_unlock_device_with_retries(mut probe: Probe, args: &Args) -> Result<Probe, RecoveryError> {
    // Set probe speed for reliable communication during recovery
    let _ = probe.set_speed(args.swd_speed);

    // Attach to unspecified target for raw AP access
    probe.attach_to_unspecified()?;

    let mut iface = probe
        .try_into_arm_debug_interface(DefaultArmSequence::create())
        .map_err(|(_p, e)| RecoveryError::ArmError(e))?;

    iface.select_debug_port(DpAddress::Default)?;

    // Step 0: Try to halt CPU to stop any running application
    halt_cpu_if_possible(iface.as_mut())?;

    const CTRL_AP: FullyQualifiedApAddress = FullyQualifiedApAddress::v1_with_default_dp(4);

    // Check initial lock state
    if !args.force {
        if let Ok(true) = check_debug_access(iface.as_mut()) {
            println!("Device already unlocked!");
            return Ok(iface.close());
        }
    }

    // Verify CTRL-AP is accessible
    let idr = iface
        .read_raw_ap_register(&CTRL_AP, IDR::ADDRESS)
        .unwrap_or(0);
    log::info!("CTRL-AP IDR: 0x{:08x}", idr);
    if idr == 0 {
        return Err(RecoveryError::UnlockError(
            "CTRL-AP not accessible, check connections".into(),
        ));
    }

    // Retry loop for unlock sequence
    for attempt in 1..=args.retries {
        println!("Unlock attempt {}/{}", attempt, args.retries);

        // Step 1: Perform rapid chip erase
        match rapid_chip_erase(iface.as_mut(), args.erase_timeout) {
            Ok(()) => {}
            Err(RecoveryError::TimeoutError) => {
                log::warn!("Erase timeout on attempt {}", attempt);
                if attempt == args.retries {
                    return Err(RecoveryError::TimeoutError);
                }
                continue;
            }
            Err(e) => return Err(e),
        }

        // Step 2: Reset device
        perform_device_reset(iface.as_mut())?;

        // Step 3: Verify unlock with timeout
        let verify_start = std::time::Instant::now();
        let verify_timeout = Duration::from_secs(2);

        while verify_start.elapsed() < verify_timeout {
            match check_debug_access(iface.as_mut()) {
                Ok(true) => {
                    println!("Device unlocked successfully!");
                    return Ok(iface.close());
                }
                Ok(false) if verify_start.elapsed() > Duration::from_millis(500) => {
                    log::warn!("Debug access not enabled after reset");
                    break;
                }
                Ok(false) => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    log::warn!("Error checking debug status: {:?}", e);
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }

        if attempt < args.retries {
            println!("Retrying unlock sequence...");
            // Longer delay between retries to allow device to stabilize
            thread::sleep(Duration::from_millis(1000));
        }
    }

    Err(RecoveryError::UnlockError(
        "Failed to unlock device after all retries".into(),
    ))
}

/// Performs a chip-wide flash erase via the NVMC `ERASEALL` register, per
/// NAN_041 §9.2.1. Equivalent in time to ~2 page erases — far faster than
/// probe-rs's default sector-by-sector erase. Requires the device to be
/// unlocked (no APPROTECT/SECUREAPPROTECT) and the CPU halted.
fn quick_chip_erase(session: &mut Session) -> Result<(), RecoveryError> {
    const NVMC_READY: u64 = 0x50039400;
    const NVMC_CONFIG: u64 = 0x50039504;
    const NVMC_ERASEALL: u64 = 0x5003950C;
    const NVMC_CONFIG_EEN: u32 = 0x2;
    const NVMC_CONFIG_REN: u32 = 0x0;

    let mut core = session.core(0)?;

    if !core.core_halted()? {
        core.halt(Duration::from_millis(500))?;
    }

    let wait_ready = |core: &mut probe_rs::Core, timeout: Duration| -> Result<(), RecoveryError> {
        let start = std::time::Instant::now();
        loop {
            if core.read_word_32(NVMC_READY)? & 0x1 == 1 {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(RecoveryError::TimeoutError);
            }
            thread::sleep(Duration::from_millis(1));
        }
    };

    core.write_word_32(NVMC_CONFIG, NVMC_CONFIG_EEN)?;
    wait_ready(&mut core, Duration::from_secs(1))?;

    let start = std::time::Instant::now();
    core.write_word_32(NVMC_ERASEALL, 1)?;
    wait_ready(&mut core, Duration::from_secs(10))?;
    log::info!("NVMC ERASEALL completed in {:?}", start.elapsed());

    core.write_word_32(NVMC_CONFIG, NVMC_CONFIG_REN)?;
    wait_ready(&mut core, Duration::from_secs(1))?;

    Ok(())
}

pub fn write_uicr(
    session: &mut Session,
    addr: u64,
    value: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    const NVMC_CONFIG: u64 = 0x50039504; // NVMC.CONFIG
    const NVMC_READY: u64 = 0x50039400; // NVMC.READY

    let mut core = session.core(0)?;

    // Step 1: Read current value and check if write is possible
    let current_value = core.read_word_32(addr)?;
    if (current_value & value) != value && current_value != 0xFFFFFFFF {
        return Err(Box::new(std::io::Error::other("Unable to write")));
    }

    // Step 2: Enable write (NVMC.CONFIG = 1)
    core.write_word_32(NVMC_CONFIG, 1)?;

    // Step 3: Wait for NVMC to be ready
    loop {
        let ready = core.read_word_32(NVMC_READY)?;
        if ready & 0x1 == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    // Step 4: Write the value
    core.write_word_32(addr, value)?;

    // Step 5: Wait for NVMC to be ready
    loop {
        let ready = core.read_word_32(NVMC_READY)?;
        if ready & 0x1 == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    // Step 6: Disable write (NVMC.CONFIG = 0)
    core.write_word_32(NVMC_CONFIG, 0)?;

    // Step 7: Wait for NVMC to be ready
    loop {
        let ready = core.read_word_32(NVMC_READY)?;
        if ready & 0x1 == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    Ok(())
}

/// Wraps `Lister::open` to translate a panic from probe-rs's Glasgow driver
/// into a `NotFound` error. probe-rs 0.31 hits `unreachable!()` in
/// `glasgow::mux::GlasgowDevice::new_from_selector` when the selector has
/// `serial_number = None` and no earlier driver opened the probe — typically
/// when no probe is plugged in. Catching it lets the timeout/retry loop
/// surface a clean error instead of an unwind.
fn open_probe_quiet(
    lister: &Lister,
    selector: DebugProbeSelector,
) -> Result<Probe, DebugProbeError> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lister.open(selector)));
    std::panic::set_hook(prev_hook);

    match result {
        Ok(r) => r,
        Err(_) => Err(DebugProbeError::ProbeCouldNotBeCreated(
            ProbeCreationError::NotFound,
        )),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args = Args::parse();

    if [args.erase_only, args.unlock_only, args.verify]
        .iter()
        .filter(|f| **f)
        .count()
        > 1
    {
        eprintln!("Error: --erase-only, --unlock-only, and --verify are mutually exclusive");
        std::process::exit(1);
    }

    // Validate arguments
    if !args.erase_only && !args.unlock_only && !args.verify {
        if let Some(ref image) = args.image {
            if !image.exists() {
                return Err(Box::new(RecoveryError::FileNotFound(
                    image.display().to_string(),
                )));
            }
        } else {
            eprintln!(
                "Error: Image file required unless --erase-only, --unlock-only, or --verify is specified"
            );
            std::process::exit(1);
        }
    }

    let lister = Lister::new();
    let start = Utc::now().timestamp_millis();

    let mut probe;

    loop {
        let selector = DebugProbeSelector {
            vendor_id: args.vendor_id,
            product_id: args.product_id,
            interface: None,
            serial_number: args.serial.clone(),
        };
        probe = match open_probe_quiet(&lister, selector) {
            Ok(p) => p,
            Err(_e) => {
                let now = Utc::now().timestamp_millis();
                if now >= start + args.timeout as i64 {
                    eprintln!("Timeout connecting to probe after {}ms", args.timeout);
                    std::process::exit(1);
                } else {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        };

        break;
    }

    println!("Got probe!");

    if args.verify {
        match verify_unlock_status(probe, &args) {
            Ok(true) => {
                println!("Device is unlocked");
                return Ok(());
            }
            Ok(false) => {
                println!("Device is locked");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Error verifying device status: {:?}", e);
                std::process::exit(2);
            }
        }
    }

    let _ = probe.set_speed(args.swd_speed);

    let probe = match try_unlock_device_with_retries(probe, &args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error unlocking device: {:?}", e);
            std::process::exit(1);
        }
    };

    let mut session = match probe.attach("nRF9151_xxAA", Permissions::new()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error attaching to device: {:?}", e);
            std::process::exit(1);
        }
    };

    println!("Created session!");

    if args.unlock_only {
        println!("Unlock-only mode: device is unlocked and accessible.");
    } else {
        println!("Performing quick chip erase via NVMC ERASEALL...");
        if let Err(e) = quick_chip_erase(&mut session) {
            eprintln!("Error during quick chip erase: {:?}", e);
            std::process::exit(1);
        }
    }

    if args.erase_only {
        println!("Erase-only mode: device unlocked and erased successfully!");
    } else if args.unlock_only {
        // Nothing more to do — skip download and UICR write.
    } else {
        let image = args.image.as_ref().unwrap(); // Safe because we validated above

        let mut options = flashing::DownloadOptions::new();
        options.skip_erase = true;

        // Flash file to device
        if let Err(e) =
            flashing::download_file_with_options(&mut session, image, Format::Hex, options)
        {
            eprintln!("Error flashing file: {:?}", e);
            std::process::exit(1);
        }

        println!("Done flashing!");

        if !args.skip_uicr {
            println!("Writing UICR protection registers...");

            if let Err(e) = write_uicr(&mut session, 0x00FF8000, 0x50FA50FA) {
                eprintln!("Error writing APPROTECT UICR: {:?}", e);
                std::process::exit(1);
            }

            if let Err(e) = write_uicr(&mut session, 0x00FF802C, 0x50FA50FA) {
                eprintln!("Error writing SECUREAPPROTECT UICR: {:?}", e);
                std::process::exit(1);
            }

            println!("UICR protection registers written successfully");
        } else {
            println!("Skipping UICR write as requested");
        }
    }

    if args.no_reset {
        println!("Skipping final reset as requested");
    } else if let Err(e) = session.core(0).and_then(|mut core| core.reset()) {
        eprintln!("Error resetting device: {:?}", e);
        std::process::exit(1);
    }

    println!("Done!");
    Ok(())
}
