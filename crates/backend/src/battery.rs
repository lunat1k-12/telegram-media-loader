//! Reads the laptop's battery charge from Linux sysfs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";

/// The system batteries, e.g. `/sys/class/power_supply/BAT0`. Batteries of
/// peripherals (wireless mice, keyboards) report `scope = Device` and are left
/// out.
pub fn find() -> io::Result<Vec<PathBuf>> {
    let mut batteries = Vec::new();
    for entry in fs::read_dir(POWER_SUPPLY_DIR)? {
        let path = entry?.path();
        if read_trimmed(&path.join("type")).as_deref() == Some("Battery")
            && read_trimmed(&path.join("scope")).as_deref() != Some("Device")
        {
            batteries.push(path);
        }
    }
    batteries.sort();
    Ok(batteries)
}

/// Charge in percent, 0–100. With more than one battery, the combined charge:
/// weighted by capacity when the batteries report it, a plain mean otherwise.
pub fn percent(batteries: &[PathBuf]) -> io::Result<u8> {
    if batteries.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no battery"));
    }

    // Some batteries report energy (µWh), others charge (µAh).
    let totals = ["energy", "charge"].iter().find_map(|unit| {
        batteries.iter().try_fold((0u64, 0u64), |(now, full), bat| {
            Some((
                now + read_u64(&bat.join(format!("{unit}_now")))?,
                full + read_u64(&bat.join(format!("{unit}_full")))?,
            ))
        })
    });
    if let Some((now, full)) = totals
        && full > 0
    {
        return Ok((now * 100 / full).min(100) as u8);
    }

    let mut sum = 0u64;
    for bat in batteries {
        let path = bat.join("capacity");
        sum += read_u64(&path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("cannot read {}", path.display())))?;
    }
    Ok((sum / batteries.len() as u64).min(100) as u8)
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}
