use chrono::Utc;
use clap::Parser;
use probe_rs::{
    architecture::arm::{
        ap::{ApRegister, CSW, IDR},
        dp::DpAddress,
        FullyQualifiedApAddress,
    },
    flashing::{self, Format},
    probe::{list::Lister, DebugProbeSelector, Probe},
    MemoryInterface, Permissions, Session,
};

use log;
use std::thread;
use std::{path::PathBuf, time::Duration};
use thiserror::Error;

#[derive(Parser)]
#[command(name = "recovery")]
#[command(about = "nRF91xx recovery tool")]
#[command(version)]
struct Args {
    #[arg(help = "Path to the hex file to flash")]
    image: PathBuf,

    #[arg(short, long, default_value_t = 2000, help = "Timeout in milliseconds for probe connection")]
    timeout: u64,

    #[arg(short, long, help = "Force unlock even if device appears unlocked")]
    force: bool,

    #[arg(long, help = "Vendor ID for debug probe", default_value_t = 0x2e8a)]
    vendor_id: u16,

    #[arg(long, help = "Product ID for debug probe", default_value_t = 0x000c)]
    product_id: u16,

    #[arg(short, long, help = "Serial number of debug probe")]
    serial: Option<String>,
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

fn try_unlock_device(mut probe: Probe, force: bool) -> Result<Probe, RecoveryError> {
    // Attach to unspecified target for raw AP access.
    probe.attach_to_unspecified()?;

    let mut iface = probe
        .try_into_arm_interface()
        .map_err(|(_p, e)| RecoveryError::DebugProbeError(e))?
        .initialize_unspecified(DpAddress::Default)
        .map_err(|(_p, e)| RecoveryError::ProbeError(e))?;

    // AP addresses (based on nRF91 docs, CTRL-AP typically at AP4).
    const APP_MEM: FullyQualifiedApAddress = FullyQualifiedApAddress::v1_with_default_dp(0); // For CSW check.
    const CTRL_AP: FullyQualifiedApAddress = FullyQualifiedApAddress::v1_with_default_dp(4); // CTRL-AP for nRF91.

    const ERASEALL: u64 = 0x004;
    const ERASEALLSTATUS: u64 = 0x008;
    const RESET: u64 = 0x000;

    // Check if locked
    let csw = iface.read_raw_ap_register(&APP_MEM, CSW::ADDRESS)?;
    let dbg_status = (csw >> 6) & 1;
    log::info!("CSW: 0x{:x}, DbgStatus: {}", csw, dbg_status);
    if dbg_status == 1 && !force {
        println!("Device already unlocked!");
        return Ok(iface.close());
    }

    // Log IDR for debugging.
    let idr = iface
        .read_raw_ap_register(&CTRL_AP, IDR::ADDRESS)
        .unwrap_or(0);
    log::info!("CTRL-AP IDR: 0x{:x}", idr);
    if idr == 0 {
        return Err(RecoveryError::UnlockError(
            "Invalid CTRL-AP IDR, check AP index".into(),
        ));
    }

    // Step 1: Erase all through CTRL-AP.
    iface.write_raw_ap_register(&CTRL_AP, ERASEALL, 1)?;
    log::info!("Started ERASEALL");

    // Wait for ERASEALLSTATUS = 0 or 15 seconds.
    let start = std::time::Instant::now();
    loop {
        let status = iface.read_raw_ap_register(&CTRL_AP, ERASEALLSTATUS)?;
        if status == 0 {
            log::info!("Erase completed");
            break;
        }
        if start.elapsed() >= Duration::from_secs(15) {
            log::info!("Erase timeout after 15s");
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }

    log::info!("Time used to erase: {:?}", start.elapsed());

    // Step 2: Reset (nRF9160: pin reset, nRF91x1: soft reset).
    // Soft reset for nRF91x1 via CTRL-AP.
    thread::sleep(Duration::from_millis(10));
    iface.write_raw_ap_register(&CTRL_AP, RESET, 1)?;
    iface.write_raw_ap_register(&CTRL_AP, RESET, 0)?;
    thread::sleep(Duration::from_millis(20));
    log::info!("Issued soft reset for nRF91x1");

    let start = std::time::Instant::now();

    loop {
        // Step 3: Check CSW DbgStatus (bit 6) on AP0.
        let csw = iface.read_raw_ap_register(&APP_MEM, CSW::ADDRESS)?;
        let dbg_status = (csw >> 6) & 1;
        log::info!("CSW: 0x{:x}, DbgStatus: {}", csw, dbg_status);
        if dbg_status == 0 && start.elapsed() > Duration::from_secs(1) {
            return Err(RecoveryError::UnlockError(
                "Debug status = 0, access port not enabled".into(),
            ));
        } else if dbg_status == 1 {
            break;
        }

        thread::sleep(Duration::from_millis(100));
    }

    println!("Unlocked device!");

    Ok(iface.close())
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
    let current_value = core.read_word_32(addr as u64)?;
    if (current_value & value) != value && current_value != 0xFFFFFFFF {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Unable to write",
        )));
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args = Args::parse();

    // Validate image file exists
    if !args.image.exists() {
        return Err(Box::new(RecoveryError::FileNotFound(
            args.image.display().to_string(),
        )));
    }

    let lister = Lister::new();
    let start = Utc::now().timestamp_millis();

    let mut probe;

    loop {
        probe = match lister.open(DebugProbeSelector {
            vendor_id: args.vendor_id,
            product_id: args.product_id,
            serial_number: args.serial.clone(),
        }) {
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

    let _ = probe.set_speed(12000);

    let probe = match try_unlock_device(probe, args.force) {
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

    let mut options = flashing::DownloadOptions::new();
    options.preverify = true;

    // Flash file to device
    if let Err(e) = flashing::download_file_with_options(&mut session, &args.image, Format::Hex, options)
    {
        eprintln!("Error flashing file: {:?}", e);
        std::process::exit(1);
    }

    println!("Done flashing!");

    if let Err(e) = write_uicr(&mut session, 0x00FF8000, 0x50FA50FA) {
        eprintln!("Error writing UICR: {:?}", e);
        std::process::exit(1);
    }

    if let Err(e) = write_uicr(&mut session, 0x00FF802C, 0x50FA50FA) {
        eprintln!("Error writing UICR: {:?}", e);
        std::process::exit(1);
    }

    // Reset with probe_rs
    session.core(0)?.reset()?;

    println!("Done!");
    Ok(())
}
