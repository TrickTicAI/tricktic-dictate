//! Measured host resources, never an assertion that a model has run successfully.
use serde::Serialize;
#[derive(Serialize)]
pub struct Hardware {
    pub architecture: &'static str,
    pub cpu_threads: usize,
    pub total_ram_bytes: Option<u64>,
    pub available_ram_bytes: Option<u64>,
    pub parakeet_cpu_supported: bool,
}
pub fn snapshot() -> Hardware {
    let (total, available) = memory();
    Hardware {
        architecture: std::env::consts::ARCH,
        cpu_threads: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        total_ram_bytes: total,
        available_ram_bytes: available,
        parakeet_cpu_supported: super::managed::engine::supported(super::managed::engine::PARAKEET),
    }
}
#[cfg(windows)]
fn memory() -> (Option<u64>, Option<u64>) {
    #[repr(C)]
    struct Status {
        length: u32,
        load: u32,
        total: u64,
        available: u64,
        total_page: u64,
        available_page: u64,
        total_virtual: u64,
        available_virtual: u64,
        extended: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(status: *mut Status) -> i32;
    }
    let mut status = Status {
        length: std::mem::size_of::<Status>() as u32,
        load: 0,
        total: 0,
        available: 0,
        total_page: 0,
        available_page: 0,
        total_virtual: 0,
        available_virtual: 0,
        extended: 0,
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
        (Some(status.total), Some(status.available))
    } else {
        (None, None)
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
fn memory() -> (Option<u64>, Option<u64>) {
    let Ok(memory) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let value = |name: &str| {
        memory.lines().find_map(|line| {
            line.strip_prefix(name)
                .and_then(|v| v.split_whitespace().next())
                .and_then(|n| n.parse::<u64>().ok())
                .and_then(|v| v.checked_mul(1024))
        })
    };
    (value("MemTotal:"), value("MemAvailable:"))
}

#[cfg(target_os = "macos")]
fn memory() -> (Option<u64>, Option<u64>) {
    fn sysctl(name: &str) -> Option<u64> {
        let output = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", name])
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        std::str::from_utf8(&output.stdout)
            .ok()?
            .trim()
            .parse()
            .ok()
    }
    // macOS has unified memory. Free pages are a conservative snapshot, not a
    // promise about reclaimable caches or how much Metal can allocate.
    let total = sysctl("hw.memsize");
    let available = sysctl("vm.page_free_count")
        .zip(sysctl("hw.pagesize"))
        .and_then(|(pages, size)| pages.checked_mul(size));
    (total, available)
}
