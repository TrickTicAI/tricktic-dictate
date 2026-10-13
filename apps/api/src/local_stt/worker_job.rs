//! A private Windows job replaces the ownership formerly inherited from the C#
//! backend host. Closing the parent process kills only this owned worker tree.
#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min: usize,
        max: usize,
        active: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Io {
        read_ops: u64,
        write_ops: u64,
        other_ops: u64,
        read_bytes: u64,
        write_bytes: u64,
        other_bytes: u64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Extended {
        basic: Basic,
        io: Io,
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> isize;
        fn SetInformationJobObject(job: isize, class: i32, info: *const c_void, length: u32)
            -> i32;
        fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
    }
    pub struct WorkerJob(isize);
    impl WorkerJob {
        pub fn attach(process_id: u32) -> Option<Self> {
            // SAFETY: initialized C layouts, borrowed buffers, and owned handles. No
            // inherited handle lets the child keep its own job alive after parent exit.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job == 0 {
                    return None;
                }
                let guard = Self(job);
                let mut info = Extended::default();
                info.basic.flags = 0x2000;
                if SetInformationJobObject(
                    job,
                    9,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<Extended>() as u32,
                ) == 0
                {
                    return None;
                }
                // Model/codec workers need neither clipboard access nor desktop
                // control. Leave GPU display/USER-handle access unrestricted.
                // Assignment occurs after spawn; this is defense in depth, not
                // a pre-exec security sandbox for an untrusted executable.
                let ui_restrictions: u32 = 0x2 | 0x4 | 0x8 | 0x40 | 0x80;
                if SetInformationJobObject(
                    job,
                    4, // JobObjectBasicUIRestrictions
                    &ui_restrictions as *const _ as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                ) == 0
                {
                    return None;
                }
                let process = OpenProcess(0x0100 | 0x0001, 0, process_id);
                if process == 0 {
                    return None;
                }
                let result = AssignProcessToJobObject(job, process);
                CloseHandle(process);
                if result == 0 {
                    None
                } else {
                    Some(guard)
                }
            }
        }
    }
    impl Drop for WorkerJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
#[cfg(windows)]
pub use windows::WorkerJob;

#[cfg(target_os = "macos")]
pub struct WorkerJob(i32);
#[cfg(target_os = "macos")]
impl WorkerJob {
    pub fn attach(pid: u32) -> Option<Self> {
        i32::try_from(pid).ok().filter(|pid| *pid > 1).map(Self)
    }
}
#[cfg(target_os = "macos")]
impl Drop for WorkerJob {
    fn drop(&mut self) {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // This process group was created by our own worker host.
        unsafe {
            kill(-self.0, 9);
        }
    }
}
