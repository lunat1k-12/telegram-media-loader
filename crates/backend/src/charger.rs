//! Keeps the laptop battery between 30% and 80% by switching the smart plug
//! its charger is plugged into.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use shared::PowerStatus;
use tokio::time::MissedTickBehavior;

use crate::{battery, plug};

const CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// Above this charge the plug is turned off.
const STOP_ABOVE: u8 = 80;
/// Below this charge the plug is turned on.
const START_BELOW: u8 = 30;

/// The reading from the latest check, shared with the web API. It stays at the
/// all-`None` default on a machine without a battery.
pub type StatusHandle = Arc<Mutex<PowerStatus>>;

/// Checks the battery every `CHECK_INTERVAL` until the process is stopped.
/// Returns at once on a machine without a battery.
pub async fn run(status: StatusHandle) {
    let batteries = match battery::find() {
        Ok(batteries) if !batteries.is_empty() => batteries,
        Ok(_) => {
            log::warn!("no battery found in /sys/class/power_supply — charge control is disabled");
            return;
        }
        Err(err) => {
            log::warn!("cannot read /sys/class/power_supply ({err}) — charge control is disabled");
            return;
        }
    };
    log::info!(
        "charge control: keeping the battery between {START_BELOW}% and {STOP_ABOVE}%, checking every {}s",
        CHECK_INTERVAL.as_secs()
    );

    let mut ticker = tokio::time::interval(CHECK_INTERVAL);
    // After a suspend, check once on resume instead of catching up on every missed tick.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let batteries = batteries.clone();
        let status = Arc::clone(&status);
        match tokio::task::spawn_blocking(move || check(&batteries, &status)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => log::warn!("charge control: {err:#}"),
            Err(err) => log::error!("charge control check panicked: {err}"),
        }
    }
}

/// One check: reads the battery and the plug, publishes both for the web UI,
/// and, when the battery is outside the band, makes sure the plug is in the
/// state that brings it back.
fn check(batteries: &[PathBuf], status: &StatusHandle) -> anyhow::Result<()> {
    // Published in one go once the check is over, failed or not, so the UI
    // never shows half of a check — and shows "unknown" for whatever failed.
    let mut reading = PowerStatus::default();
    let result = check_and_switch(batteries, &mut reading);
    *status.lock().unwrap_or_else(PoisonError::into_inner) = reading;
    result
}

fn check_and_switch(batteries: &[PathBuf], reading: &mut PowerStatus) -> anyhow::Result<()> {
    let percent = battery::percent(batteries)?;
    reading.battery_percent = Some(percent);
    let plug_on = plug::is_on()?;
    reading.plug_on = Some(plug_on);

    let Some(want_on) = wanted_plug_state(percent) else {
        log::debug!("charge control: battery {percent}%, within range");
        return Ok(());
    };
    if plug_on == want_on {
        log::debug!("charge control: battery {percent}%, plug already {}", on_off(want_on));
        return Ok(());
    }

    // If the switch fails the plug is in an unknown state until the next check.
    reading.plug_on = None;
    plug::switch(want_on)?;
    reading.plug_on = Some(want_on);
    log::info!("charge control: battery {percent}%, plug switched {}", on_off(want_on));
    Ok(())
}

/// `Some(false)` above the band, `Some(true)` below it, `None` inside it.
fn wanted_plug_state(percent: u8) -> Option<bool> {
    if percent > STOP_ABOVE {
        Some(false)
    } else if percent < START_BELOW {
        Some(true)
    } else {
        None
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "ON" } else { "OFF" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        assert_eq!(wanted_plug_state(100), Some(false));
        assert_eq!(wanted_plug_state(81), Some(false));
        assert_eq!(wanted_plug_state(80), None);
        assert_eq!(wanted_plug_state(30), None);
        assert_eq!(wanted_plug_state(29), Some(true));
        assert_eq!(wanted_plug_state(0), Some(true));
    }
}
