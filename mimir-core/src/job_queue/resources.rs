//! Best-effort per-job resource enforcement (OS-specific).
//!
//! The [`ResourceGuard`] applies CPU affinity, `nice` level, and (on Linux)
//! a cgroup v2 memory cap for the duration of a job run, then restores the
//! previous state on drop. Every operation is best-effort: unsupported
//! platforms, missing permissions, or unwritable cgroup filesystems degrade
//! to a debug log and the job runs without the limit (issue #91). The cgroup
//! memory cap is process-wide: the whole process is moved into the job
//! cgroup, so the cap applies to the entire daemon while the job runs.

#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(unix)]
use tracing::debug;

use super::JobResourceLimits;

/// Applies per-job resource limits and restores the previous state on drop.
///
/// The guard must be created and dropped on the same thread (the job's
/// dedicated run thread) so affinity and nice values are restored exactly.
pub struct ResourceGuard {
    #[cfg(target_os = "linux")]
    saved_affinity: Option<nix::sched::CpuSet>,
    #[cfg(unix)]
    saved_nice: Option<i32>,
    #[cfg(target_os = "linux")]
    cgroup: Option<CgroupSnapshot>,
}

impl ResourceGuard {
    /// Apply `limits` for the current thread/process, remembering the
    /// previous state so it can be restored when the guard is dropped.
    pub fn apply(limits: JobResourceLimits, job_id: &str) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            saved_affinity: limits.cpu_cores.and_then(apply_cpu_affinity),
            #[cfg(unix)]
            saved_nice: limits.nice_level.and_then(apply_nice),
            #[cfg(target_os = "linux")]
            cgroup: limits
                .memory_limit_bytes
                .and_then(|bytes| apply_memory_limit(bytes, job_id)),
        }
    }
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(saved) = self.saved_affinity.take() {
            if let Err(e) = nix::sched::sched_setaffinity(nix::unistd::Pid::from_raw(0), &saved) {
                debug!(error = %e, "failed to restore CPU affinity after job");
            }
        }
        #[cfg(unix)]
        if let Some(saved) = self.saved_nice.take() {
            if let Err(e) = rustix::process::setpriority_process(None, saved) {
                debug!(error = %e, "failed to restore nice value after job");
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(cgroup) = self.cgroup.take() {
            cgroup.restore();
        }
    }
}

/// Narrow the calling thread's CPU affinity to at most `cores` CPUs,
/// preferring the lowest-numbered CPUs it is already allowed to run on.
/// Returns the previous affinity mask so it can be restored on drop.
#[cfg(target_os = "linux")]
fn apply_cpu_affinity(cores: u8) -> Option<nix::sched::CpuSet> {
    use nix::sched::{CpuSet, sched_getaffinity, sched_setaffinity};
    use nix::unistd::Pid;

    if cores == 0 {
        return None;
    }
    let saved = sched_getaffinity(Pid::from_raw(0)).ok()?;
    let mut narrowed = CpuSet::new();
    let mut kept = 0u8;
    for cpu in 0..CpuSet::count() {
        if kept >= cores {
            break;
        }
        if saved.is_set(cpu).ok()? {
            narrowed.set(cpu).ok()?;
            kept += 1;
        }
    }
    if kept == 0 {
        return None;
    }
    match sched_setaffinity(Pid::from_raw(0), &narrowed) {
        Ok(()) => Some(saved),
        Err(e) => {
            debug!(error = %e, cores, "failed to apply CPU affinity for job");
            None
        }
    }
}

/// Set the calling thread's `nice` value to `target`, returning the previous
/// value so it can be restored on drop. Lowering the nice value (making the
/// thread more urgent) requires privileges and is skipped when denied.
#[cfg(unix)]
fn apply_nice(target: i8) -> Option<i32> {
    use rustix::process::{getpriority_process, setpriority_process};

    // `nice` is only defined for -20..=19; clamp out-of-range values rather
    // than silently dropping the limit.
    let target = i32::from(target).clamp(-20, 19);
    let current = getpriority_process(None).ok()?;
    if current == target {
        return None;
    }
    match setpriority_process(None, target) {
        Ok(()) => Some(current),
        Err(e) => {
            debug!(error = %e, target, "failed to apply nice level for job");
            None
        }
    }
}

/// State needed to move the process back out of a job cgroup on drop.
#[cfg(target_os = "linux")]
struct CgroupSnapshot {
    parent: PathBuf,
    child: PathBuf,
}

#[cfg(target_os = "linux")]
impl CgroupSnapshot {
    fn restore(self) {
        let pid = std::process::id().to_string();
        if let Err(e) = std::fs::write(self.parent.join("cgroup.procs"), &pid) {
            debug!(
                error = %e,
                path = %self.parent.display(),
                "failed to move process back to parent cgroup"
            );
        }
        if let Err(e) = std::fs::remove_dir(&self.child) {
            debug!(
                error = %e,
                path = %self.child.display(),
                "failed to remove job cgroup"
            );
        }
    }
}

/// Best-effort cgroup v2 memory cap: create a per-job child cgroup under the
/// process's own cgroup, write `memory.max`, and move the process into it for
/// the duration of the run. Skipped entirely when the cgroup filesystem is
/// not writable (no delegation).
#[cfg(target_os = "linux")]
fn apply_memory_limit(bytes: u64, job_id: &str) -> Option<CgroupSnapshot> {
    if bytes == 0 {
        return None;
    }
    let parent = own_cgroup_v2_dir()?;
    let child = parent.join(format!(
        "mimir-job-{}-{}",
        std::process::id(),
        sanitize_cgroup_name(job_id)
    ));
    if let Err(e) = std::fs::create_dir(&child) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            debug!(
                error = %e,
                path = %child.display(),
                "failed to create job cgroup; skipping memory limit"
            );
            return None;
        }
        // A stale directory from a previously crashed run is reused.
    }
    if let Err(e) = std::fs::write(child.join("memory.max"), bytes.to_string()) {
        debug!(
            error = %e,
            path = %child.display(),
            "failed to set cgroup memory.max; skipping memory limit"
        );
        let _ = std::fs::remove_dir(&child);
        return None;
    }
    let pid = std::process::id().to_string();
    if let Err(e) = std::fs::write(child.join("cgroup.procs"), &pid) {
        debug!(
            error = %e,
            path = %child.display(),
            "failed to move process into job cgroup; skipping memory limit"
        );
        let _ = std::fs::remove_dir(&child);
        return None;
    }
    Some(CgroupSnapshot { parent, child })
}

/// Resolve the process's cgroup v2 directory under `/sys/fs/cgroup`.
#[cfg(target_os = "linux")]
fn own_cgroup_v2_dir() -> Option<PathBuf> {
    let contents = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = parse_cgroup_v2_path(&contents)?;
    Some(PathBuf::from("/sys/fs/cgroup").join(path))
}

/// Parse the unified (v2) hierarchy path from `/proc/self/cgroup` contents.
///
/// Lines look like `0::/user.slice/...`; the unified hierarchy is the one
/// with an empty controller list (`0`).
#[cfg(target_os = "linux")]
fn parse_cgroup_v2_path(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (controllers, rest) = line.split_once(':')?;
        if controllers == "0" {
            let path = rest.trim_start_matches(':');
            (!path.is_empty()).then(|| path.to_string())
        } else {
            None
        }
    })
}

/// cgroup directory names may only contain `[a-z0-9._-]`; job ids are
/// already lowercase dotted identifiers, but sanitize defensively.
#[cfg(target_os = "linux")]
fn sanitize_cgroup_name(job_id: &str) -> String {
    job_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parse_cgroup_v2_path_extracts_unified_hierarchy() {
        let contents =
            "12:memory:/user.slice/legacy\n0::/user.slice/user-1000.slice/session.scope\n";
        assert_eq!(
            parse_cgroup_v2_path(contents).as_deref(),
            Some("/user.slice/user-1000.slice/session.scope")
        );
    }

    #[test]
    fn parse_cgroup_v2_path_returns_none_without_unified_line() {
        let contents = "12:memory:/user.slice/legacy\n";
        assert_eq!(parse_cgroup_v2_path(contents), None);
    }

    #[test]
    fn sanitize_cgroup_name_keeps_valid_chars() {
        assert_eq!(
            sanitize_cgroup_name("knowledge.optimization"),
            "knowledge.optimization"
        );
        assert_eq!(
            sanitize_cgroup_name("bad/name:with*chars"),
            "bad_name_with_chars"
        );
    }

    #[test]
    fn resource_guard_applies_and_restores_limits() {
        use nix::sched::{CpuSet, sched_getaffinity};
        use nix::unistd::Pid;
        use rustix::process::getpriority_process;

        let original_affinity = match sched_getaffinity(Pid::from_raw(0)) {
            Ok(set) => set,
            Err(_) => return, // best-effort: environment may not allow it
        };
        let original_nice = match getpriority_process(None) {
            Ok(nice) => nice,
            Err(_) => return,
        };
        // Lowering the nice value needs privileges; skip when the target is
        // more urgent than the current value.
        if original_nice > 10 {
            return;
        }

        let limits = JobResourceLimits {
            cpu_cores: Some(1),
            nice_level: Some(10),
            memory_limit_bytes: None,
        };
        let guard = ResourceGuard::apply(limits, "test.limits");

        let narrowed = sched_getaffinity(Pid::from_raw(0)).unwrap();
        let mut count = 0usize;
        for cpu in 0..CpuSet::count() {
            if narrowed.is_set(cpu).unwrap_or(false) {
                count += 1;
            }
        }
        assert_eq!(count, 1);
        assert_eq!(getpriority_process(None).unwrap(), 10);

        drop(guard);

        let restored = sched_getaffinity(Pid::from_raw(0)).unwrap();
        for cpu in 0..CpuSet::count() {
            assert_eq!(
                restored.is_set(cpu).unwrap_or(false),
                original_affinity.is_set(cpu).unwrap_or(false)
            );
        }
        // Restoring a raised nice value requires privileges; unprivileged
        // processes can only keep or raise it further, so assert the value
        // never becomes more urgent than the original.
        assert!(getpriority_process(None).unwrap() >= original_nice);
    }
}
