//! A cached, native host-process admission signal. Sampling never forks, and
//! admission only reads the last service-owned sample.

use std::{
    ffi::CString,
    os::raw::{c_char, c_int, c_uint, c_void},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use services::services::container::ContainerError;
use tokio_util::sync::CancellationToken;

const MIN_RESERVE: usize = 256;
const DEFAULT_RESERVE: usize = 320;
const SAMPLE_MAX_AGE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessPressure {
    user_processes: usize,
    user_zombies: usize,
    system_processes: usize,
    system_zombies: usize,
    user_limit: usize,
    system_limit: usize,
}

#[derive(Debug, Clone)]
struct CachedPressure {
    sampled_at: Instant,
    result: Result<ProcessPressure, String>,
}

#[derive(Clone)]
pub(crate) struct HostProcessBudget {
    cached: Arc<RwLock<CachedPressure>>,
    reserve: usize,
}

impl HostProcessBudget {
    pub(crate) fn start(shutdown: CancellationToken) -> Self {
        let budget = Self {
            cached: Arc::new(RwLock::new(CachedPressure {
                sampled_at: Instant::now(),
                result: sample_process_pressure(),
            })),
            reserve: configured_reserve(),
        };
        let refresher = budget.clone();
        let _refresh_task = spawn_refresher(shutdown, move || refresher.refresh());
        budget
    }

    pub(crate) fn ensure_available(&self) -> Result<(), ContainerError> {
        let cached = self
            .cached
            .read()
            .expect("process budget cache lock poisoned")
            .clone();
        if cached.sampled_at.elapsed() > SAMPLE_MAX_AGE {
            return Err(ContainerError::InfrastructureUnavailable(
                "host process sample is stale".to_string(),
            ));
        }
        let pressure = cached
            .result
            .map_err(ContainerError::InfrastructureUnavailable)?;
        if pressure.user_processes.saturating_add(self.reserve) < pressure.user_limit
            && pressure.system_processes.saturating_add(self.reserve) < pressure.system_limit
        {
            return Ok(());
        }
        Err(ContainerError::InfrastructureUnavailable(format!(
            "host process reserve unavailable (user {}/{} including {} zombies; system {}/{} including {} zombies; reserve {})",
            pressure.user_processes,
            pressure.user_limit,
            pressure.user_zombies,
            pressure.system_processes,
            pressure.system_limit,
            pressure.system_zombies,
            self.reserve,
        )))
    }

    fn refresh(&self) {
        *self
            .cached
            .write()
            .expect("process budget cache lock poisoned") = CachedPressure {
            sampled_at: Instant::now(),
            result: sample_process_pressure(),
        };
    }
}

fn spawn_refresher(
    shutdown: CancellationToken,
    mut refresh: impl FnMut() + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => refresh(),
            }
        }
    })
}

fn configured_reserve() -> usize {
    std::env::var("CDESKTOP_PROCESS_RESERVE")
        .ok()
        .and_then(|value| value.parse().ok())
        .map(|value: usize| value.max(MIN_RESERVE))
        .unwrap_or(DEFAULT_RESERVE)
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcBsdShortInfo {
    pid: u32,
    _ppid: u32,
    _pgid: u32,
    status: u32,
    _comm: [c_char; 16],
    _flags: u32,
    uid: u32,
    _gid: u32,
    _ruid: u32,
    _rgid: u32,
    _svuid: u32,
    _svgid: u32,
    _reserved: u32,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn proc_listpids(
        kind: c_uint,
        typeinfo: c_uint,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn getuid() -> u32;
    fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
}

#[cfg(target_os = "macos")]
fn sysctl_usize(name: &str) -> Result<usize, String> {
    let name = CString::new(name).map_err(|_| "invalid sysctl name".to_string())?;
    let mut value: c_int = 0;
    let mut size = std::mem::size_of_val(&value);
    // SAFETY: buffers and lengths match the documented sysctlbyname ABI.
    let result = unsafe {
        sysctlbyname(
            name.as_ptr(),
            &mut value as *mut c_int as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || value <= 0 {
        return Err(format!("cannot sample {name:?}"));
    }
    Ok(value as usize)
}

#[cfg(target_os = "macos")]
fn list_pids_with<F>(mut list: F) -> Result<Vec<c_int>, String>
where
    F: FnMut(*mut c_void, c_int) -> c_int,
{
    let mut size = list(std::ptr::null_mut(), 0);
    if size <= 0 {
        return Err("cannot sample process ids".to_string());
    }
    for _ in 0..3 {
        let mut pids = vec![0_i32; size as usize / std::mem::size_of::<c_int>() + 64];
        let capacity = (pids.len() * std::mem::size_of::<c_int>()) as c_int;
        let bytes = list(pids.as_mut_ptr().cast(), capacity);
        if bytes < 0 {
            return Err("cannot enumerate process ids".to_string());
        }
        // A full buffer could have raced a growing process table. Retry with a
        // fresh size rather than silently admitting from an undercount.
        if bytes < capacity {
            pids.truncate(bytes as usize / std::mem::size_of::<c_int>());
            return Ok(pids);
        }
        size = list(std::ptr::null_mut(), 0);
        if size <= 0 {
            return Err("cannot resample process ids".to_string());
        }
    }
    Err("process table changed while sampling".to_string())
}

#[cfg(target_os = "macos")]
fn sample_process_pressure() -> Result<ProcessPressure, String> {
    const PROC_ALL_PIDS: c_uint = 1;
    const PROC_PIDT_SHORTBSDINFO: c_int = 13;
    const SZOMB: u32 = 5;
    let pids = list_pids_with(|buffer, size| unsafe {
        // SAFETY: libproc accepts a null sizing buffer or the supplied PID buffer.
        proc_listpids(PROC_ALL_PIDS, 0, buffer, size)
    })?;
    // SAFETY: getuid has no arguments and no side effects.
    let uid = unsafe { getuid() };
    let mut pressure = ProcessPressure {
        user_processes: 0,
        user_zombies: 0,
        system_processes: 0,
        system_zombies: 0,
        user_limit: sysctl_usize("kern.maxprocperuid")?,
        system_limit: sysctl_usize("kern.maxproc")?,
    };
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        let mut info = std::mem::MaybeUninit::<ProcBsdShortInfo>::zeroed();
        // SAFETY: info has the exact ABI layout and supplied size for this flavor.
        let read = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDT_SHORTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<ProcBsdShortInfo>() as c_int,
            )
        };
        if read != std::mem::size_of::<ProcBsdShortInfo>() as c_int {
            continue;
        }
        // SAFETY: libproc initialized the complete value above.
        let info = unsafe { info.assume_init() };
        let zombie = info.status == SZOMB;
        pressure.system_processes += 1;
        pressure.system_zombies += usize::from(zombie);
        if info.uid == uid {
            pressure.user_processes += 1;
            pressure.user_zombies += usize::from(zombie);
        }
    }
    Ok(pressure)
}

#[cfg(target_os = "linux")]
fn sample_process_pressure() -> Result<ProcessPressure, String> {
    let uid = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("cannot sample self uid: {error}"))?
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| "cannot parse self uid".to_string())?;
    let mut pressure = ProcessPressure {
        user_processes: 0,
        user_zombies: 0,
        system_processes: 0,
        system_zombies: 0,
        user_limit: linux_user_limit()?,
        system_limit: linux_system_limit()?,
    };
    for entry in
        std::fs::read_dir("/proc").map_err(|error| format!("cannot enumerate /proc: {error}"))?
    {
        let entry = entry.map_err(|error| format!("cannot read /proc: {error}"))?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let status = match std::fs::read_to_string(entry.path().join("status")) {
            Ok(status) => status,
            Err(_) => continue,
        };
        let process_uid = status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|value| value.parse::<u32>().ok());
        let zombie = status
            .lines()
            .find_map(|line| line.strip_prefix("State:"))
            .is_some_and(|state| state.trim_start().starts_with('Z'));
        pressure.system_processes += 1;
        pressure.system_zombies += usize::from(zombie);
        if process_uid == Some(uid) {
            pressure.user_processes += 1;
            pressure.user_zombies += usize::from(zombie);
        }
    }
    Ok(pressure)
}

#[cfg(target_os = "linux")]
fn linux_user_limit() -> Result<usize, String> {
    let limits = std::fs::read_to_string("/proc/self/limits")
        .map_err(|error| format!("cannot read process limits: {error}"))?;
    let value = limits
        .lines()
        .find_map(|line| line.strip_prefix("Max processes"))
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| "cannot parse Max processes".to_string())?;
    if value == "unlimited" {
        Ok(usize::MAX)
    } else {
        value
            .parse()
            .map_err(|_| "cannot parse Max processes".to_string())
    }
}

#[cfg(target_os = "linux")]
fn linux_system_limit() -> Result<usize, String> {
    std::fs::read_to_string("/proc/sys/kernel/pid_max")
        .map_err(|error| format!("cannot read pid_max: {error}"))?
        .trim()
        .parse()
        .map_err(|_| "cannot parse pid_max".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn sample_process_pressure() -> Result<ProcessPressure, String> {
    Err("native process instrumentation is unavailable on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_is_conservative_even_when_configured_lower() {
        assert!(configured_reserve() >= MIN_RESERVE);
        assert!(DEFAULT_RESERVE > 250);
    }

    #[test]
    fn pressure_rejects_before_the_reserve_is_consumed() {
        let budget = HostProcessBudget {
            cached: Arc::new(RwLock::new(CachedPressure {
                sampled_at: Instant::now(),
                result: Ok(ProcessPressure {
                    user_processes: 2_410,
                    user_zombies: 31,
                    system_processes: 2_410,
                    system_zombies: 31,
                    user_limit: 2_666,
                    system_limit: 10_000,
                }),
            })),
            reserve: 320,
        };
        assert!(matches!(
            budget.ensure_available(),
            Err(ContainerError::InfrastructureUnavailable(_))
        ));
    }

    #[test]
    fn unavailable_sample_rejects_as_infrastructure() {
        let budget = HostProcessBudget {
            cached: Arc::new(RwLock::new(CachedPressure {
                sampled_at: Instant::now(),
                result: Err("libproc unavailable".to_string()),
            })),
            reserve: DEFAULT_RESERVE,
        };
        assert!(matches!(
            budget.ensure_available(),
            Err(ContainerError::InfrastructureUnavailable(_))
        ));
    }

    #[test]
    fn stale_sample_rejects_as_infrastructure() {
        let budget = HostProcessBudget {
            cached: Arc::new(RwLock::new(CachedPressure {
                sampled_at: Instant::now() - SAMPLE_MAX_AGE - Duration::from_secs(1),
                result: Err("ignored".to_string()),
            })),
            reserve: DEFAULT_RESERVE,
        };
        assert!(matches!(
            budget.ensure_available(),
            Err(ContainerError::InfrastructureUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn refresher_stops_when_its_service_shuts_down() {
        let shutdown = CancellationToken::new();
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = ticks.clone();
        let handle = spawn_refresher(shutdown.clone(), move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        shutdown.cancel();
        handle.await.unwrap();
        let before = ticks.load(std::sync::atomic::Ordering::SeqCst);
        tokio::task::yield_now().await;
        assert_eq!(ticks.load(std::sync::atomic::Ordering::SeqCst), before);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_sampler_has_a_separate_global_limit() {
        let pressure = sample_process_pressure().unwrap();
        assert!(pressure.system_limit >= pressure.system_processes);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn growing_process_table_is_never_silently_undercounted() {
        let mut calls = 0;
        let result = list_pids_with(|_, size| {
            calls += 1;
            if size == 0 { 4 } else { size }
        });
        assert!(result.is_err());
        assert!(calls >= 6);
    }
}
