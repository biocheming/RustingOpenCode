//! Global process registry for tracking child processes spawned by the application.
//!
//! Provides a singleton [`ProcessRegistry`] that plugins, tools, and agents
//! use to register/unregister their OS processes. The TUI reads this registry
//! to display a live process panel in the sidebar.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use parking_lot::RwLock;

/// Global singleton process registry.
static REGISTRY: Lazy<ProcessRegistry> = Lazy::new(ProcessRegistry::new);

/// Returns a reference to the global [`ProcessRegistry`].
pub fn global_registry() -> &'static ProcessRegistry {
    &REGISTRY
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessKind {
    Plugin,
    Bash,
    Agent,
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub kind: ProcessKind,
    pub started_at: i64,
    pub cpu_percent: f32,
    pub memory_kb: u64,
}

pub struct ProcessRegistry {
    processes: RwLock<HashMap<u32, ProcessInfo>>,
    /// Previous CPU jiffies snapshot for delta-based CPU% calculation.
    prev_cpu: RwLock<HashMap<u32, (u64, u64)>>,
}

impl ProcessRegistry {
    fn new() -> Self {
        Self {
            processes: RwLock::new(HashMap::new()),
            prev_cpu: RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, pid: u32, name: String, kind: ProcessKind) {
        let info = ProcessInfo {
            pid,
            name,
            kind,
            started_at: chrono::Utc::now().timestamp(),
            cpu_percent: 0.0,
            memory_kb: 0,
        };
        self.processes.write().insert(pid, info);
    }

    pub fn unregister(&self, pid: u32) {
        self.processes.write().remove(&pid);
        self.prev_cpu.write().remove(&pid);
    }

    pub fn list(&self) -> Vec<ProcessInfo> {
        self.processes.read().values().cloned().collect()
    }

    /// Send SIGTERM, wait briefly, then SIGKILL if the process is still alive.
    pub fn kill(&self, pid: u32) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        {
            use std::io::{Error, ErrorKind};
            // SIGTERM
            let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if ret != 0 {
                let err = Error::last_os_error();
                if err.kind() != ErrorKind::PermissionDenied {
                    self.unregister(pid);
                }
                return Err(err);
            }
            // Brief wait then SIGKILL (best-effort, non-blocking)
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            });
            self.unregister(pid);
            Ok(())
        }
        #[cfg(not(unix))]
        {
            self.unregister(pid);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "kill not supported on this platform",
            ))
        }
    }

    /// Refresh CPU and memory stats by reading `/proc/<pid>/stat` and `/proc/<pid>/status`.
    /// Sums stats across the entire child process tree so that e.g. bun's worker
    /// threads are included in the parent plugin's numbers.
    pub fn refresh_stats(&self) {
        let pids: Vec<u32> = self.processes.read().keys().copied().collect();
        let mut stale = Vec::new();

        for pid in pids {
            match read_proc_tree_stats(pid) {
                Some((cpu_ticks, mem_kb)) => {
                    let cpu_percent = self.compute_cpu_percent(pid, cpu_ticks);
                    if let Some(info) = self.processes.write().get_mut(&pid) {
                        info.cpu_percent = cpu_percent;
                        info.memory_kb = mem_kb;
                    }
                }
                None => {
                    // Process no longer exists
                    stale.push(pid);
                }
            }
        }

        for pid in stale {
            self.unregister(pid);
        }
    }

    fn compute_cpu_percent(&self, pid: u32, current_ticks: u64) -> f32 {
        let mut prev = self.prev_cpu.write();
        let total_now = read_total_cpu_ticks().unwrap_or(1);
        let (prev_ticks, prev_total) = prev.get(&pid).copied().unwrap_or((0, total_now));
        prev.insert(pid, (current_ticks, total_now));

        let dticks = current_ticks.saturating_sub(prev_ticks) as f64;
        let dtotal = total_now.saturating_sub(prev_total).max(1) as f64;
        ((dticks / dtotal) * 100.0) as f32
    }
}

// ---------------------------------------------------------------------------
// /proc helpers (Linux only)
// ---------------------------------------------------------------------------

/// Read utime+stime from `/proc/<pid>/stat` and VmRSS from `/proc/<pid>/status`.
/// Returns `(cpu_ticks, memory_kb)` or `None` if the process is gone.
fn read_proc_stats(pid: u32) -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
        // Fields after the comm (which may contain spaces/parens):
        // find closing ')' then split the rest.
        let after_comm = stat.rfind(')')? + 2;
        let fields: Vec<&str> = stat[after_comm..].split_whitespace().collect();
        // field index 11 = utime, 12 = stime (0-indexed after comm)
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;

        let status = std::fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
        let mem_kb = status
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .and_then(|l| {
                l.split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .unwrap_or(0);

        Some((utime + stime, mem_kb))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Some((0, 0))
    }
}

/// Total CPU ticks from `/proc/stat` (sum of all fields on the first `cpu` line).
fn read_total_cpu_ticks() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let cpu_line = stat.lines().next()?;
        let total: u64 = cpu_line
            .split_whitespace()
            .skip(1) // skip "cpu"
            .filter_map(|v| v.parse::<u64>().ok())
            .sum();
        Some(total)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some(1)
    }
}

/// Sum CPU ticks and memory across the entire process tree rooted at `pid`.
/// This captures child workers (e.g. bun spawning threads/subprocesses).
fn read_proc_tree_stats(pid: u32) -> Option<(u64, u64)> {
    let root_stats = read_proc_stats(pid)?;
    let children = collect_descendant_pids(pid);
    let mut total_ticks = root_stats.0;
    let mut total_mem = root_stats.1;
    for child_pid in children {
        if let Some((ticks, mem)) = read_proc_stats(child_pid) {
            total_ticks += ticks;
            total_mem += mem;
        }
    }
    Some((total_ticks, total_mem))
}

/// Recursively collect all descendant PIDs by scanning /proc/*/stat for matching ppid.
fn collect_descendant_pids(root_pid: u32) -> Vec<u32> {
    #[cfg(target_os = "linux")]
    {
        let mut result = Vec::new();
        let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return result;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid_str) = name.to_str() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            if pid == root_pid {
                continue;
            }
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
                if let Some(ppid) = parse_parent_pid_from_stat(&stat) {
                    children_by_parent.entry(ppid).or_default().push(pid);
                }
            }
        }

        let mut queue = vec![root_pid];
        let mut seen: HashSet<u32> = HashSet::new();
        while let Some(parent) = queue.pop() {
            let Some(children) = children_by_parent.get(&parent) else {
                continue;
            };
            for &child in children {
                if child == root_pid || !seen.insert(child) {
                    continue;
                }
                result.push(child);
                queue.push(child);
            }
        }
        result
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root_pid;
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn parse_parent_pid_from_stat(stat: &str) -> Option<u32> {
    let after_comm = stat.rfind(')')? + 2;
    let fields: Vec<&str> = stat[after_comm..].split_whitespace().collect();
    fields.get(1)?.parse::<u32>().ok()
}
