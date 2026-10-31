//! Cooperative cancellation for verification and its child process group.
use anyhow::{Context, Result};
use command_group::CommandGroup;
use std::{
    process::{Command, ExitStatus},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

/// Install a process-wide signal handler for the lifetime of this CLI run.
pub struct SignalGuard {
    cancelled: Arc<AtomicBool>,
}

impl SignalGuard {
    pub fn install() -> Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
            .context("installing verification cancellation handler")?;
        Ok(Self { cancelled })
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.cancelled
    }
}

/// Spawn a separate process group (a Windows job object), and kill and reap it
/// before returning on cancellation. Detached children can escape this group.
pub fn run(command: &mut Command, cancelled: &AtomicBool) -> Result<ExitStatus> {
    anyhow::ensure!(!cancelled.load(Ordering::SeqCst), "verification cancelled");
    let mut child = command.group_spawn().context("starting verification child process")?;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            // A child may exit concurrently with cancellation. Always reap it.
            if let Err(error) = child.kill() {
                if child.try_wait()?.is_none() {
                    return Err(error).context("stopping verification process group");
                }
            }
            return child.wait().context("reaping cancelled verification process group");
        }
        if let Some(status) = child.try_wait().context("waiting for verification child process")? {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
#[path = "verify_signals/tests.rs"]
mod tests;
