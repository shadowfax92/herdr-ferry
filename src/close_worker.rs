//! The popup publishes confirmed work, then asks Herdr to launch a plugin
//! action. Herdr owns that process, so closing the popup's/source's terminals
//! cannot kill the executor. Atomic claims prevent duplicate action invocations
//! from replaying destructive work; interrupted claims are never auto-resumed.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::close_ops::{execute, CloseReport};
use crate::close_plan::ClosePlan;
use crate::herdr::Herdr;

const CONFIRMATION_TTL_SECS: u64 = 60;

#[derive(Debug)]
pub struct Ticket {
    pub id: String,
    pub report_path: PathBuf,
}

#[derive(Debug)]
pub enum JobStatus {
    Pending,
    Running,
    Finished(Box<CloseReport>),
    Revoked,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimedJob {
    pub id: String,
    socket: String,
    confirmed_at: u64,
    pub plan: ClosePlan,
}

/// Private per-plugin storage, explicitly scoped by session socket on every
/// claim. The state directory may be shared by several named Herdr sessions.
pub struct JobStore {
    directory: PathBuf,
    socket: String,
}

impl JobStore {
    pub fn new(directory: impl Into<PathBuf>, socket: &str) -> Result<Self> {
        ensure!(
            !socket.is_empty(),
            "HERDR_SOCKET_PATH is required for safe handoff"
        );
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            directory,
            socket: socket.into(),
        })
    }

    pub fn from_environment() -> Result<Self> {
        let directory = PathBuf::from(
            std::env::var_os("HERDR_PLUGIN_STATE_DIR")
                .context("HERDR_PLUGIN_STATE_DIR is not set")?,
        )
        .join("close-jobs");
        Self::new(
            directory,
            &std::env::var("HERDR_SOCKET_PATH").context("HERDR_SOCKET_PATH is not set")?,
        )
    }

    pub fn enqueue(&self, plan: &ClosePlan) -> Result<Ticket> {
        let id = Uuid::new_v4().to_string();
        let job = ClaimedJob {
            id: id.clone(),
            socket: self.socket.clone(),
            confirmed_at: now(),
            plan: plan.clone(),
        };
        self.write_atomic(&id, "pending.json", &serde_json::to_vec(&job)?)?;
        Ok(Ticket {
            report_path: self.path(&id, "report.txt"),
            id,
        })
    }

    pub fn revoke(&self, id: &str) -> Result<bool> {
        check_id(id)?;
        match fs::remove_file(self.path(id, "pending.json")) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub fn claim(&self) -> Result<Vec<ClaimedJob>> {
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let Some(id) = path
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_suffix(".pending.json"))
            else {
                continue;
            };
            check_id(id)?;
            // Another worker/revocation may win between enumeration and read.
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let job: ClaimedJob = serde_json::from_slice(&bytes)?;
            ensure!(
                job.id == id,
                "Confirmed job identity does not match its file"
            );
            if job.socket != self.socket {
                continue;
            }
            match fs::rename(&path, self.path(id, "running.json")) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            }
            if now().saturating_sub(job.confirmed_at) > CONFIRMATION_TTL_SECS {
                self.finish(
                    &job,
                    &CloseReport {
                        failed: job.plan.panes.len(),
                        errors: vec![
                            "Confirmation expired before handoff; open Ferry and review again"
                                .into(),
                        ],
                        ..CloseReport::default()
                    },
                )?;
                continue;
            }
            jobs.push(job);
        }
        Ok(jobs)
    }

    pub fn finish(&self, job: &ClaimedJob, report: &CloseReport) -> Result<()> {
        let body = format!(
            "{}\n{}\n{}\n",
            job.plan.kind.title(),
            report.summary(),
            report.errors.join("\n")
        );
        self.write_atomic(&job.id, "report.txt", body.as_bytes())?;
        self.write_atomic(&job.id, "result.json", &serde_json::to_vec_pretty(report)?)?;
        // Retain the claimed snapshot for diagnosis. Its name is never scanned
        // as pending, including after server or worker restarts.
        Ok(())
    }

    pub fn status(&self, id: &str) -> Result<JobStatus> {
        check_id(id)?;
        if self.path(id, "result.json").is_file() {
            return Ok(JobStatus::Finished(serde_json::from_slice(&fs::read(
                self.path(id, "result.json"),
            )?)?));
        }
        if self.path(id, "running.json").is_file() {
            return Ok(JobStatus::Running);
        }
        if self.path(id, "pending.json").is_file() {
            return Ok(JobStatus::Pending);
        }
        Ok(JobStatus::Revoked)
    }

    fn path(&self, id: &str, suffix: &str) -> PathBuf {
        self.directory.join(format!("{id}.{suffix}"))
    }

    fn write_atomic(&self, id: &str, suffix: &str, bytes: &[u8]) -> Result<()> {
        check_id(id)?;
        let temp = self.directory.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(temp, self.path(id, suffix))?;
        Ok(())
    }
}

fn check_id(id: &str) -> Result<()> {
    Uuid::parse_str(id).context("Invalid close job ID")?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn submit(herdr: &Herdr, plan: &ClosePlan) -> Result<Ticket> {
    let store = JobStore::from_environment()?;
    let ticket = store.enqueue(plan)?;
    if let Err(error) = herdr.start_close_worker() {
        if store.revoke(&ticket.id)? {
            return Err(error.context("Could not hand off confirmed work; nothing was closed"));
        }
        // An uncertain CLI response cannot revoke a claim already accepted by
        // the server-owned worker. Return its ticket rather than offer a retry.
    }
    Ok(ticket)
}

pub fn run_from_environment() -> Result<()> {
    let herdr = Herdr::from_environment();
    let store = JobStore::from_environment()?;
    for job in store.claim()? {
        let report = execute(&herdr, &job.plan);
        store.finish(&job, &report)?;
        let path = store.path(&job.id, "report.txt");
        println!("{}\nReport: {}", report.summary(), path.display());
        let detail = report.errors.first().cloned().unwrap_or_default();
        let _ = herdr.notify(&format!(
            "{}: {}. {}",
            job.plan.kind.title(),
            report.summary(),
            detail
        ));
    }
    Ok(())
}

pub fn report_path(ticket: &Ticket) -> &Path {
    &ticket.report_path
}
