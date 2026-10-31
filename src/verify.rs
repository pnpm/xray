//! Verify proposed repairs against the user's command and retain a minimal set.
use crate::{
    analyze::Scope,
    declaration_patch, discovery, report, trace,
    verify_candidates::{self, Candidate},
    verify_lease::Lease,
    verify_signals::{self, SignalGuard},
    verify_transaction::Snapshot,
    workspace,
};
use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct Options<'a> {
    pub project: &'a Path,
    pub workspace: &'a Path,
    pub command: &'a [OsString],
    pub scope: Scope,
    pub regenerate_types: bool,
    pub max_attempts: usize,
    pub max_candidates: usize,
}

struct Session<'a> {
    options: Options<'a>,
    snapshot: Snapshot,
    managed: Vec<PathBuf>,
    dirty: bool,
    attempts: usize,
    considered: BTreeSet<String>,
    recovery: Option<PathBuf>,
    lease: Option<Lease>,
    cancelled: &'a AtomicBool,
}

pub fn run(options: Options<'_>) -> Result<ExitCode> {
    let signals = SignalGuard::install()?;
    eprintln!("Verification baseline: running the command before changing settings.");
    let packages = discovery::installed_packages(options.workspace)?;
    let baseline =
        trace::run_cancellable(options.command, &packages, options.project, signals.flag())?;
    if signals.flag().load(Ordering::SeqCst) {
        return Ok(ExitCode::from(130));
    }
    if baseline.status.success() {
        eprintln!("Verification baseline succeeded; no repairs were made.");
        return Ok(ExitCode::SUCCESS);
    }
    show_observations(&baseline);
    let initial_status = trace::status_code(baseline.status);
    let importers = workspace::projects(options.workspace)?;
    let snapshot = Snapshot::capture(options.workspace, &importers)?;
    let mut managed = snapshot.lockfiles();
    if !managed.iter().any(|path| path.is_file()) {
        bail!("verified repair requires an existing pnpm lockfile to restore the dependency graph");
    }
    managed.push(snapshot.workspace_file());
    let mut session = Session {
        options,
        snapshot,
        managed,
        dirty: false,
        attempts: 0,
        considered: BTreeSet::new(),
        recovery: None,
        lease: None,
        cancelled: signals.flag(),
    };
    let result = session.search(baseline.reports).and_then(|retained| {
        session.snapshot.verify_unchanged()?;
        Ok(retained)
    });
    match result {
        Ok(Some(retained)) => {
            session.remove_recovery()?;
            eprintln!(
                "Verified repair retained {} necessary proposals after {} attempts.",
                retained.len(),
                session.attempts,
            );
            for candidate in retained {
                eprintln!("  {}", candidate.key);
            }
            Ok(ExitCode::SUCCESS)
        }
        Ok(None) => {
            if let Err(error) = session.rollback() {
                session.report_recovery();
                return Err(error);
            }
            session.remove_recovery()?;
            eprintln!("No verified repair found; original workspace settings, lockfiles, and dependency graph restored.");
            Ok(ExitCode::from(initial_status))
        }
        Err(error) => {
            let cancelled = session.cancelled.load(Ordering::SeqCst);
            if let Err(rollback) = session.rollback() {
                session.report_recovery();
                return Err(
                    error.context(format!("rollback could not finish safely: {rollback:#}"))
                );
            }
            session.remove_recovery()?;
            if cancelled {
                eprintln!("Verification interrupted; original dependency graph restored.");
                return Ok(ExitCode::from(130));
            }
            Err(error)
        }
    }
}

impl Session<'_> {
    fn search(
        &mut self,
        mut observed: Vec<report::PackageReport>,
    ) -> Result<Option<Vec<Candidate>>> {
        let mut applied = Vec::new();
        while self.attempts < self.options.max_attempts
            && self.considered.len() < self.options.max_candidates
        {
            self.check_cancelled()?;
            self.snapshot.verify_unchanged()?;
            let packages = discovery::installed_packages(self.options.workspace)?;
            let candidates = verify_candidates::collect(
                self.options.project,
                self.options.workspace,
                &packages,
                &observed,
                self.options.scope,
                self.options.regenerate_types,
            )?;
            let Some(candidate) = candidates.into_iter().find(|candidate| {
                !self
                    .considered
                    .contains(&format!("{}\n{}", candidate.key, candidate.configuration))
                    && !applied.iter().any(|existing: &Candidate| existing.key == candidate.key)
            }) else {
                break;
            };
            self.considered.insert(format!("{}\n{}", candidate.key, candidate.configuration));
            eprintln!("Verification proposal: {}", candidate.key);
            self.register_artifacts(&candidate)?;
            let mut trial = applied.clone();
            trial.push(candidate);
            let installed = self.configure(&trial, false)?;
            self.check_cancelled()?;
            if !installed {
                eprintln!("Proposal install failed; restoring the preceding graph without changing pnpm policies.");
                if !self.configure(&applied, applied.is_empty())? {
                    bail!("could not reinstall the preceding dependency graph");
                }
                continue;
            }
            self.attempts += 1;
            let result = trace::run_cancellable(
                self.options.command,
                &discovery::installed_packages(self.options.workspace)?,
                self.options.project,
                self.cancelled,
            )?;
            self.check_cancelled()?;
            self.snapshot.verify_unchanged()?;
            if result.status.success() {
                return self.minimize(trial);
            }
            show_observations(&result);
            observed = result.reports;
            applied = trial;
        }
        if self.attempts >= self.options.max_attempts
            || self.considered.len() >= self.options.max_candidates
        {
            eprintln!("Verified repair search reached its configured limit.");
        }
        Ok(None)
    }

    fn minimize(&mut self, mut retained: Vec<Candidate>) -> Result<Option<Vec<Candidate>>> {
        let mut index = 0;
        while index < retained.len() {
            self.check_cancelled()?;
            if self.attempts >= self.options.max_attempts {
                eprintln!("Verification limit prevents proving a minimal repair; rolling back.");
                return Ok(None);
            }
            let mut trial = retained.clone();
            let removed = trial.remove(index);
            eprintln!("Checking whether {} is necessary.", removed.key);
            let installed = self.configure(&trial, trial.is_empty())?;
            self.check_cancelled()?;
            let succeeded = if installed {
                self.attempts += 1;
                let result = trace::run_cancellable(
                    self.options.command,
                    &discovery::installed_packages(self.options.workspace)?,
                    self.options.project,
                    self.cancelled,
                )?;
                self.check_cancelled()?;
                self.snapshot.verify_unchanged()?;
                result.status.success()
            } else {
                false
            };
            if succeeded {
                retained = trial;
                index = 0;
            } else {
                if !self.configure(&retained, false)? {
                    bail!("could not reinstall the verified dependency graph");
                }
                index += 1;
            }
        }
        // Recheck the exact graph that will remain after minimization, rather
        // than relying on a command result from an earlier graph installation.
        if self.attempts >= self.options.max_attempts {
            return Ok(None);
        }
        self.attempts += 1;
        let final_result = trace::run_cancellable(
            self.options.command,
            &discovery::installed_packages(self.options.workspace)?,
            self.options.project,
            self.cancelled,
        )?;
        self.check_cancelled()?;
        self.snapshot.verify_unchanged()?;
        Ok(final_result.status.success().then_some(retained))
    }

    fn register_artifacts(&mut self, candidate: &Candidate) -> Result<()> {
        for path in candidate.artifacts.keys() {
            let path = self.snapshot.track_artifact(path)?;
            if !self.managed.contains(&path) {
                self.managed.push(path);
            }
        }
        if self.recovery.is_some() {
            self.protect_recovery()?;
        }
        Ok(())
    }

    fn configure(&mut self, candidates: &[Candidate], frozen: bool) -> Result<bool> {
        self.check_cancelled()?;
        self.protect_recovery()?;
        self.snapshot.restore()?;
        self.checkpoint(&self.managed.clone())?;
        let plan = verify_candidates::compose(candidates)?;
        self.dirty = true;
        if !plan.artifacts.is_empty() {
            self.snapshot.ensure_patch_directory()?;
            self.protect_recovery()?;
        }
        declaration_patch::apply(&self.snapshot.workspace_file(), &plan.configuration, &plan)?;
        let mut changed = vec![self.snapshot.workspace_file()];
        changed.extend(
            self.managed
                .iter()
                .filter(|path| path.extension().is_some_and(|extension| extension == "patch"))
                .cloned(),
        );
        self.checkpoint(&changed)?;
        let installed = install(self.options.workspace, frozen, self.cancelled)?;
        self.checkpoint(&self.snapshot.lockfiles())?;
        self.snapshot.verify_unchanged()?;
        Ok(installed)
    }

    fn rollback(&mut self) -> Result<()> {
        self.cancelled.store(false, Ordering::SeqCst);
        self.snapshot.restore()?;
        self.checkpoint(&self.managed.clone())?;
        if self.dirty {
            eprintln!("Reinstalling the original locked dependency graph.");
            let restored = install(self.options.workspace, true, self.cancelled)?;
            self.checkpoint(&self.snapshot.lockfiles())?;
            self.snapshot.verify_unchanged()?;
            // In separate-lockfile workspaces an empty root can acquire a new
            // lockfile on reinstall. Preserve its original absence as well.
            self.snapshot.restore()?;
            self.checkpoint(&self.managed.clone())?;
            if !restored {
                bail!("original configuration and locks restored, but pnpm could not restore dependency links; run pnpm install");
            }
        }
        Ok(())
    }

    fn check_cancelled(&self) -> Result<()> {
        anyhow::ensure!(!self.cancelled.load(Ordering::SeqCst), "verification interrupted");
        Ok(())
    }

    fn checkpoint(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.snapshot.checkpoint(paths)?;
        if self.recovery.is_some() {
            self.protect_recovery()?;
        }
        Ok(())
    }

    fn protect_recovery(&mut self) -> Result<()> {
        if let Some(path) = &self.recovery {
            self.snapshot.refresh_recovery(path)?;
        } else {
            let path = self.snapshot.save_recovery()?;
            let lease = Lease::acquire(&path)?;
            eprintln!("Recovery journal: {}", path.display());
            self.recovery = Some(path);
            self.lease = Some(lease);
        }
        Ok(())
    }

    fn remove_recovery(&mut self) -> Result<()> {
        if let Some(path) = &self.recovery {
            Snapshot::remove_recovery(path)?;
            if let Some(lease) = self.lease.take() {
                lease.remove()?;
            }
            match std::fs::remove_dir(path) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty,
                    ) => {}
                Err(error) => return Err(error).context("removing completed recovery journal"),
            }
            self.recovery = None;
        }
        Ok(())
    }

    fn report_recovery(&self) {
        if let Some(path) = &self.recovery {
            eprintln!("Recovery journal retained at {}. Resolve intervening edits before running xray <workspace> --recover <journal>.", path.display());
        }
    }
}

fn install(workspace: &Path, frozen: bool, cancelled: &AtomicBool) -> Result<bool> {
    let mut command = crate::process::command(std::ffi::OsStr::new("pnpm"));
    command
        .arg("install")
        .arg(if frozen { "--frozen-lockfile" } else { "--no-frozen-lockfile" })
        .current_dir(workspace);
    let status = verify_signals::run(&mut command, cancelled)
        .context("running pnpm install under existing project policies")?;
    Ok(status.success())
}

pub fn recover(project: &Path, journal: &Path) -> Result<ExitCode> {
    let lease = Lease::acquire(journal)?;
    let mut snapshot = Snapshot::load_recovery(journal)?;
    if project.canonicalize()? != snapshot.workspace_root() {
        bail!(
            "recovery workspace does not match the selected project: {}",
            snapshot.workspace_root().display(),
        );
    }
    let signals = SignalGuard::install()?;
    let paths = snapshot.tracked_paths();
    snapshot.restore()?;
    snapshot.checkpoint(&paths)?;
    snapshot.refresh_recovery(journal)?;
    let installed = install(snapshot.workspace_root(), true, signals.flag())?;
    snapshot.checkpoint(&snapshot.lockfiles())?;
    snapshot.refresh_recovery(journal)?;
    snapshot.verify_unchanged()?;
    snapshot.restore()?;
    snapshot.checkpoint(&paths)?;
    snapshot.refresh_recovery(journal)?;
    if !installed {
        bail!("original files restored but dependency reinstall failed; recovery journal retained at {}", journal.display());
    }
    Snapshot::remove_recovery(journal)?;
    lease.remove()?;
    match std::fs::remove_dir(journal) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty,
            ) => {}
        Err(error) => return Err(error).context("removing recovered journal"),
    }
    eprintln!("Recovered original workspace settings, lockfiles, and dependency graph.");
    Ok(ExitCode::SUCCESS)
}

fn show_observations(result: &trace::TraceResult) {
    if !result.reports.is_empty() {
        eprint!("{}", report::as_text(&result.reports));
    }
    for diagnostic in &result.diagnostics {
        eprintln!("Observed {}: {}: {}", diagnostic.package, diagnostic.file, diagnostic.message);
    }
}
