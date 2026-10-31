//! Cooperative cancellation for verification and its child process group.
use anyhow::{Context, Result};
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

const POLL: Duration = Duration::from_millis(25);

/// Spawn a separate process group, and kill and reap it before returning on
/// cancellation. Detached children can escape this group.
///
/// When Xray owns the terminal, the child group becomes the terminal's
/// foreground group for its lifetime, so commands that read input or change
/// terminal modes are not stopped by `SIGTTIN` or `SIGTTOU`. Terminal
/// interrupts then reach the child instead of Xray, so a child that dies from
/// one marks the run as cancelled. A child stopped for terminal access that
/// cannot be given the terminal is killed instead of waited on forever.
#[cfg(unix)]
pub fn run(command: &mut Command, cancelled: &AtomicBool) -> Result<ExitStatus> {
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
    anyhow::ensure!(!cancelled.load(Ordering::SeqCst), "verification cancelled");
    let terminal = unix::Terminal::controlling();
    let child = command.process_group(0).spawn().context("starting verification child process")?;
    let pid = libc::pid_t::try_from(child.id()).context("invalid child process id")?;
    // The child is reaped below through `waitpid`, never through `Child`.
    drop(child);
    let mut group = unix::Group { pid, terminal, foreground: false };
    group.take_terminal();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            group.kill();
            let status = unix::wait(pid, 0)?.context("reaping cancelled verification process")?;
            return Ok(ExitStatus::from_raw(status));
        }
        match unix::wait(pid, libc::WNOHANG | libc::WUNTRACED)? {
            None => thread::sleep(POLL),
            Some(status) if libc::WIFSTOPPED(status) => {
                group.stopped(libc::WSTOPSIG(status))?;
            }
            Some(status) => {
                let status = ExitStatus::from_raw(status);
                if group.foreground && unix::interrupted(status) {
                    cancelled.store(true, Ordering::SeqCst);
                }
                return Ok(status);
            }
        }
    }
}

/// Spawn a Windows job object, and kill and reap it before returning on
/// cancellation. Detached children can escape this group.
#[cfg(not(unix))]
pub fn run(command: &mut Command, cancelled: &AtomicBool) -> Result<ExitStatus> {
    use command_group::CommandGroup;
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
        thread::sleep(POLL);
    }
}

#[cfg(unix)]
mod unix {
    use anyhow::{bail, Context as _, Result};
    use std::{
        io,
        os::{fd::RawFd, unix::process::ExitStatusExt as _},
        process::ExitStatus,
    };

    /// The controlling terminal, as reached through a standard stream.
    pub struct Terminal {
        fd: RawFd,
    }

    impl Terminal {
        pub fn controlling() -> Option<Self> {
            [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO]
                .into_iter()
                // SAFETY: isatty only inspects the descriptor number.
                .find(|&fd| unsafe { libc::isatty(fd) } == 1)
                .map(|fd| Self { fd })
        }

        fn owner(&self) -> libc::pid_t {
            // SAFETY: tcgetpgrp only inspects the descriptor.
            unsafe { libc::tcgetpgrp(self.fd) }
        }

        /// Hand the terminal to `group`. A background caller must block
        /// `SIGTTOU`, or the kernel stops it for changing the foreground.
        fn set_owner(&self, group: libc::pid_t) -> bool {
            // SAFETY: the signal sets are initialized by sigemptyset before
            // use, and the previous mask is restored before returning.
            unsafe {
                let mut block = std::mem::zeroed::<libc::sigset_t>();
                let mut previous = std::mem::zeroed::<libc::sigset_t>();
                libc::sigemptyset(&raw mut block);
                libc::sigaddset(&raw mut block, libc::SIGTTOU);
                libc::pthread_sigmask(libc::SIG_BLOCK, &raw const block, &raw mut previous);
                let result = libc::tcsetpgrp(self.fd, group);
                libc::pthread_sigmask(libc::SIG_SETMASK, &raw const previous, std::ptr::null_mut());
                result == 0
            }
        }
    }

    pub struct Group {
        pub pid: libc::pid_t,
        pub terminal: Option<Terminal>,
        /// The child group currently owns the terminal.
        pub foreground: bool,
    }

    impl Group {
        /// Give the terminal to the child group only while Xray is in the
        /// foreground, never taking it from a shell that backgrounded Xray.
        pub fn take_terminal(&mut self) -> bool {
            let Some(terminal) = &self.terminal else { return false };
            // SAFETY: setpgid and getpgrp have no memory-safety requirements.
            // Repeating the child's own setpgid closes the race with exec;
            // its error after exec is expected and harmless.
            let ours = unsafe {
                libc::setpgid(self.pid, self.pid);
                libc::getpgrp()
            };
            let owner = terminal.owner();
            if owner == self.pid || (owner == ours && terminal.set_owner(self.pid)) {
                self.foreground = true;
            }
            self.foreground
        }

        /// Return the terminal to Xray if the child group still holds it or
        /// has exited while holding it.
        fn release_terminal(&mut self) {
            if !self.foreground {
                return;
            }
            self.foreground = false;
            let Some(terminal) = &self.terminal else { return };
            let owner = terminal.owner();
            // SAFETY: getpgrp has no requirements; kill with signal 0 only
            // checks whether the process group still exists.
            let (ours, orphaned) = unsafe {
                (
                    libc::getpgrp(),
                    owner > 0
                        && libc::kill(-owner, 0) == -1
                        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
                )
            };
            if owner == self.pid || (owner != ours && orphaned) {
                terminal.set_owner(ours);
            }
        }

        pub fn kill(&mut self) {
            // SAFETY: the group id belongs to a child that has not been reaped.
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
            self.release_terminal();
        }

        fn resume(&self) {
            // SAFETY: the group id belongs to a child that has not been reaped.
            unsafe {
                libc::kill(-self.pid, libc::SIGCONT);
            }
        }

        /// Handle a stopped child instead of waiting for an exit that cannot
        /// happen until something continues it.
        pub fn stopped(&mut self, signal: i32) -> Result<()> {
            if signal == libc::SIGTTIN || signal == libc::SIGTTOU {
                if self.take_terminal() {
                    self.resume();
                    return Ok(());
                }
                self.kill();
                wait(self.pid, 0)?;
                bail!("verification command was stopped for terminal access while xray does not own the terminal; run xray in the foreground");
            }
            if self.foreground {
                // Job control: Ctrl-Z stopped the child; suspend Xray as well
                // and hand the terminal back to the child once continued.
                self.release_terminal();
                // SAFETY: stopping the current process has no memory effects.
                unsafe {
                    libc::kill(libc::getpid(), libc::SIGTSTP);
                }
                self.take_terminal();
                self.resume();
            }
            Ok(())
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            self.release_terminal();
        }
    }

    /// `waitpid` that retries interruptions; `None` means no state change.
    pub fn wait(pid: libc::pid_t, options: i32) -> Result<Option<i32>> {
        loop {
            let mut status = 0;
            // SAFETY: status points to a live integer.
            let result = unsafe { libc::waitpid(pid, &raw mut status, options) };
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error).context("waiting for verification child process");
            }
            return Ok((result != 0).then_some(status));
        }
    }

    /// A child that dies from a terminal interrupt, or exits with the
    /// conventional status for one, was interrupted by the user.
    pub fn interrupted(status: ExitStatus) -> bool {
        matches!(status.signal(), Some(libc::SIGINT | libc::SIGQUIT)) || status.code() == Some(130)
    }
}

#[cfg(test)]
#[path = "verify_signals/tests.rs"]
mod tests;
