//! Reserve a logical CPU before creating any runtime/solver threads.
use std::{io, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationCpuPolicy {
    Auto,
    Off,
    Cpu(usize),
}

impl FromStr for ValidationCpuPolicy {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            value => value.parse().map(Self::Cpu).map_err(|_| {
                "TROUTE_VALIDATION_CPU must be auto, off, or a logical CPU ID".to_owned()
            }),
        }
    }
}

/// Returns the reserved CPU. Call on the startup thread before creating Tokio:
/// every ordinary child thread/process inherits the mask excluding this CPU.
/// Automatic mode preserves native single-CPU/non-Linux development startup,
/// with an explicit warning; an explicitly requested CPU must be reservable.
pub fn reserve_validation_cpu(policy: ValidationCpuPolicy) -> io::Result<Option<usize>> {
    if policy == ValidationCpuPolicy::Off {
        return Ok(None);
    }
    let allowed = match allowed_cpus() {
        Ok(cpus) if cpus.len() >= 2 => cpus,
        result if policy == ValidationCpuPolicy::Auto => {
            crate::logging::logger().startup_warning(&format!(
                "fast validation uses its dedicated thread without CPU affinity: {result:?}; provide at least two allowed Linux CPUs for core reservation"
            ));
            return Ok(None);
        }
        Ok(_) => {
            return Err(io::Error::other(
                "a separate validation CPU requires at least two allowed CPUs",
            ))
        }
        Err(error) => return Err(error),
    };
    let cpu = match policy {
        ValidationCpuPolicy::Cpu(cpu) if allowed.contains(&cpu) => cpu,
        ValidationCpuPolicy::Cpu(_) => {
            return Err(io::Error::other(
                "TROUTE_VALIDATION_CPU is outside the allowed CPU set",
            ))
        }
        ValidationCpuPolicy::Auto => *allowed.last().expect("at least two allowed CPUs"),
        ValidationCpuPolicy::Off => unreachable!(),
    };
    let ordinary: Vec<_> = allowed
        .into_iter()
        .filter(|&candidate| candidate != cpu)
        .collect();
    set_current_cpus(&ordinary)?;
    Ok(Some(cpu))
}

pub(crate) fn pin_current_thread(cpu: usize) -> io::Result<()> {
    set_current_cpus(&[cpu])
}

#[cfg(target_os = "linux")]
fn allowed_cpus() -> io::Result<Vec<usize>> {
    // SAFETY: cpu_set_t is plain C data; the buffer is correctly sized and
    // initialized before libc writes the calling thread's affinity mask.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&set), &mut set) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((0..libc::CPU_SETSIZE as usize)
        .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &set) })
        .collect())
}

#[cfg(target_os = "linux")]
fn set_current_cpus(cpus: &[usize]) -> io::Result<()> {
    if cpus.is_empty() || cpus.iter().any(|&cpu| cpu >= libc::CPU_SETSIZE as usize) {
        return Err(io::Error::other("invalid CPU affinity mask"));
    }
    // SAFETY: all indices are checked against cpu_set_t's capacity, and the
    // correctly sized mask lives through the calling-thread-only syscall.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for &cpu in cpus {
        unsafe { libc::CPU_SET(cpu, &mut set) };
    }
    if unsafe { libc::sched_setaffinity(0, std::mem::size_of_val(&set), &set) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if allowed_cpus()? != cpus {
        return Err(io::Error::other(
            "kernel did not apply the requested CPU affinity mask",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn allowed_cpus() -> io::Result<Vec<usize>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "CPU reservation is supported on Linux, including Docker's Linux VM",
    ))
}

#[cfg(not(target_os = "linux"))]
fn set_current_cpus(_cpus: &[usize]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "CPU affinity is unavailable on this platform",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn dedicated_cpu_is_excluded_from_ordinary_child_threads() {
        std::thread::spawn(|| {
            let allowed = allowed_cpus().unwrap();
            if allowed.len() < 2 {
                return;
            }
            let cpu = *allowed.last().unwrap();
            assert_eq!(
                reserve_validation_cpu(ValidationCpuPolicy::Cpu(cpu)).unwrap(),
                Some(cpu)
            );
            let ordinary = allowed_cpus().unwrap();
            assert!(!ordinary.contains(&cpu));
            assert_eq!(
                std::thread::spawn(allowed_cpus).join().unwrap().unwrap(),
                ordinary
            );
            std::thread::spawn(move || {
                pin_current_thread(cpu).unwrap();
                assert_eq!(allowed_cpus().unwrap(), vec![cpu]);
            })
            .join()
            .unwrap();
        })
        .join()
        .unwrap();
    }

    #[test]
    fn invalid_cpu_is_rejected_and_single_cpu_auto_mode_is_explicitly_unpinned() {
        std::thread::spawn(|| {
            let allowed = allowed_cpus().unwrap();
            assert!(
                reserve_validation_cpu(ValidationCpuPolicy::Cpu(libc::CPU_SETSIZE as usize))
                    .is_err()
            );
            assert_eq!(allowed_cpus().unwrap(), allowed);
            pin_current_thread(allowed[0]).unwrap();
            assert!(reserve_validation_cpu(ValidationCpuPolicy::Cpu(allowed[0])).is_err());
            assert_eq!(
                reserve_validation_cpu(ValidationCpuPolicy::Auto).unwrap(),
                None
            );
        })
        .join()
        .unwrap();
    }
}
