use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const FRESH_SAMPLE: Duration = Duration::from_millis(400);
const SAMPLE_MIN_AGE: Duration = Duration::from_secs(1);
const SAMPLE_MAX_AGE: Duration = Duration::from_secs(120);

pub fn in_container() -> bool {
    ["/.dockerenv", "/run/.containerenv"]
        .iter()
        .any(|path| Path::new(path).exists())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ServerLoad {
    pub cpu_percent: Option<f64>,
    pub cores: Option<u32>,
    pub load: Option<[f64; 3]>,
    pub memory: Option<Usage>,
    pub disk: Option<Usage>,
    pub uptime_seconds: Option<u64>,
    pub app_memory_bytes: Option<u64>,
    pub database_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

pub fn parse_cpu(stat: &str) -> Option<(CpuTimes, u32)> {
    let mut times = None;
    let mut cores = 0u32;
    for line in stat.lines() {
        if let Some(rest) = line.strip_prefix("cpu ") {
            let values: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            if values.len() >= 4 {
                let idle = values[3] + values.get(4).copied().unwrap_or(0);
                let total: u64 = values.iter().take(8).sum();
                times = Some(CpuTimes {
                    busy: total.saturating_sub(idle),
                    total,
                });
            }
        } else if line.starts_with("cpu") && line.as_bytes().get(3).is_some_and(u8::is_ascii_digit)
        {
            cores += 1;
        }
    }
    times.map(|t| (t, cores))
}

pub fn cpu_percent(before: CpuTimes, after: CpuTimes) -> Option<f64> {
    let total = after.total.checked_sub(before.total)?;
    let busy = after.busy.checked_sub(before.busy)?;
    if total == 0 {
        return None;
    }
    Some((busy as f64 * 100.0 / total as f64).clamp(0.0, 100.0))
}

fn kib_field(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(name)?.strip_prefix(':')?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
}

pub fn parse_meminfo(text: &str) -> Option<Usage> {
    let total = kib_field(text, "MemTotal")?.checked_mul(1024)?;
    let available = kib_field(text, "MemAvailable")?.checked_mul(1024)?;
    Some(Usage {
        used_bytes: total.saturating_sub(available),
        total_bytes: total,
    })
}

pub fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let mut parts = text.split_whitespace().map(|v| v.parse::<f64>().ok());
    Some([parts.next()??, parts.next()??, parts.next()??])
}

pub fn parse_uptime(text: &str) -> Option<u64> {
    let seconds = text.split_whitespace().next()?.parse::<f64>().ok()?;
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds as u64)
}

pub fn parse_rss(status: &str) -> Option<u64> {
    kib_field(status, "VmRSS")?.checked_mul(1024)
}

pub fn disk_usage(blocks: u64, free: u64, available: u64, fragment: u64) -> Option<Usage> {
    let used = blocks.checked_sub(free)?.checked_mul(fragment)?;
    let available = available.checked_mul(fragment)?;
    let total = used.checked_add(available)?;
    (total > 0).then_some(Usage {
        used_bytes: used,
        total_bytes: total,
    })
}

pub struct LoadProbe {
    proc_root: PathBuf,
    disk_path: PathBuf,
    last: Mutex<Option<(Instant, CpuTimes)>>,
}

impl LoadProbe {
    pub fn new(proc_root: impl Into<PathBuf>, disk_path: impl Into<PathBuf>) -> Self {
        LoadProbe {
            proc_root: proc_root.into(),
            disk_path: disk_path.into(),
            last: Mutex::new(None),
        }
    }

    async fn read(&self, name: &str) -> Option<String> {
        tokio::fs::read_to_string(self.proc_root.join(name))
            .await
            .ok()
    }

    fn remember(&self, times: CpuTimes) -> Option<(Instant, CpuTimes)> {
        let mut last = match self.last.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        last.replace((Instant::now(), times))
    }

    async fn cpu(&self) -> (Option<f64>, Option<u32>) {
        let Some((now, cores)) = self.read("stat").await.and_then(|s| parse_cpu(&s)) else {
            return (None, None);
        };
        let cores = (cores > 0).then_some(cores);
        let previous = self.remember(now);
        if let Some((at, before)) = previous {
            let age = at.elapsed();
            if (SAMPLE_MIN_AGE..=SAMPLE_MAX_AGE).contains(&age) {
                return (cpu_percent(before, now), cores);
            }
        }
        tokio::time::sleep(FRESH_SAMPLE).await;
        let Some((later, _)) = self.read("stat").await.and_then(|s| parse_cpu(&s)) else {
            return (None, cores);
        };
        self.remember(later);
        (cpu_percent(now, later), cores)
    }

    async fn disk(&self) -> Option<Usage> {
        let path = self.disk_path.clone();
        tokio::task::spawn_blocking(move || disk_of(&path))
            .await
            .ok()
            .flatten()
    }

    pub async fn sample(&self) -> ServerLoad {
        let (cpu_percent, cores) = self.cpu().await;
        ServerLoad {
            cpu_percent,
            cores,
            load: self.read("loadavg").await.and_then(|s| parse_loadavg(&s)),
            memory: self.read("meminfo").await.and_then(|s| parse_meminfo(&s)),
            disk: self.disk().await,
            uptime_seconds: self.read("uptime").await.and_then(|s| parse_uptime(&s)),
            app_memory_bytes: self.read("self/status").await.and_then(|s| parse_rss(&s)),
            database_bytes: None,
        }
    }
}

fn disk_of(path: &Path) -> Option<Usage> {
    let stat = rustix::fs::statvfs(path).ok()?;
    disk_usage(stat.f_blocks, stat.f_bfree, stat.f_bavail, stat.f_frsize)
}

pub fn probe() -> &'static LoadProbe {
    static PROBE: OnceLock<LoadProbe> = OnceLock::new();
    PROBE.get_or_init(|| LoadProbe::new("/proc", "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  4705 356 584 3699 23 23 0 0 0 0\ncpu0 1393 280 307 1817 12 9 0 0 0 0\ncpu1 3312 76 277 1882 11 14 0 0 0 0\nintr 1462898\n";

    #[test]
    fn процессор_считается_по_разнице_замеров() {
        let (before, cores) = parse_cpu(STAT).unwrap();
        assert_eq!(cores, 2);
        assert_eq!(before.total, 4705 + 356 + 584 + 3699 + 23 + 23);
        let after = CpuTimes {
            busy: before.busy + 30,
            total: before.total + 120,
        };
        assert_eq!(cpu_percent(before, after), Some(25.0));
        assert_eq!(cpu_percent(after, before), None);
        assert_eq!(cpu_percent(before, before), None);
    }

    #[test]
    fn память_нагрузка_и_время_работы_читаются_из_proc() {
        let meminfo = "MemTotal:        4028336 kB\nMemFree:          212456 kB\nMemAvailable:    2918672 kB\n";
        assert_eq!(
            parse_meminfo(meminfo),
            Some(Usage {
                used_bytes: (4_028_336 - 2_918_672) * 1024,
                total_bytes: 4_028_336 * 1024
            })
        );
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
        assert_eq!(
            parse_loadavg("0.42 0.38 0.30 2/412 31337\n"),
            Some([0.42, 0.38, 0.30])
        );
        assert_eq!(parse_uptime("1036800.27 3889456.01\n"), Some(1_036_800));
        assert_eq!(
            parse_rss("Name:\tstealthnet\nVmRSS:\t  123456 kB\n"),
            Some(123_456 * 1024)
        );
        assert_eq!(
            disk_usage(10_000, 4_000, 3_500, 4096),
            Some(Usage {
                used_bytes: 6_000 * 4096,
                total_bytes: 9_500 * 4096
            })
        );
        assert_eq!(disk_usage(0, 0, 0, 4096), None);
    }

    #[tokio::test]
    async fn замер_берёт_файлы_из_указанной_папки() {
        let dir = std::env::temp_dir().join(format!("sn-load-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("self")).unwrap();
        std::fs::write(dir.join("stat"), STAT).unwrap();
        std::fs::write(dir.join("loadavg"), "1.50 1.00 0.50 1/1 1\n").unwrap();
        std::fs::write(
            dir.join("meminfo"),
            "MemTotal: 2048 kB\nMemAvailable: 1024 kB\n",
        )
        .unwrap();
        std::fs::write(dir.join("uptime"), "90061.5 1.0\n").unwrap();
        std::fs::write(dir.join("self/status"), "VmRSS: 512 kB\n").unwrap();
        let probe = LoadProbe::new(&dir, &dir);
        let load = probe.sample().await;
        assert_eq!(load.cores, Some(2));
        assert_eq!(load.cpu_percent, None);
        assert_eq!(load.load, Some([1.5, 1.0, 0.5]));
        assert_eq!(load.memory.map(|m| m.used_bytes), Some(1024 * 1024));
        assert_eq!(load.uptime_seconds, Some(90_061));
        assert_eq!(load.app_memory_bytes, Some(512 * 1024));
        assert!(load.disk.is_some_and(|d| d.total_bytes > 0));
        std::fs::remove_dir_all(&dir).ok();
    }
}
