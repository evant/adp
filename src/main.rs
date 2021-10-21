#[cfg(test)]
#[macro_use]
extern crate derive_builder;

use std::fmt::Debug;
use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::exit;

use ambassador::Delegate;
use anyhow::{anyhow, Context};
use named_semaphore::{Semaphore, SemaphoreGuard};
use tracing::{debug, info, instrument};
use tracing_subscriber::FmtSubscriber;

use exitstatus::{ExitStatusError, ExitStatusExt};

use crate::filelock::{FileLockGuard, FileLockGuardExt};
use crate::lockfile::LockFileEntries;
use crate::runtime::{Pid, RealRuntime, Runtime, Serial};

mod filelock;
mod exitstatus;
mod lockfile;
mod adb;
mod runtime;

type Result<T = ()> = std::result::Result<T, anyhow::Error>;

fn main() {
    debug_log();

    let result = run();

    match result {
        Ok(_) => {
            // success!
        }
        Err(e) => {
            eprintln!("{}", e);
            let error_code = e.downcast::<ExitStatusError>()
                .ok()
                .and_then(|e| e.code()).unwrap_or(1);
            exit(error_code);
        }
    }
}

fn debug_log() {
    if cfg!(debug_assertions) {
        let _ = FmtSubscriber::builder()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(true)
            .try_init();
    }
}

#[instrument]
fn run() -> Result {
    let (cmd, args) = {
        let mut args = std::env::args().skip(1);
        (args.next().ok_or(anyhow!("missing command"))?, args)
    };

    // TODO: allow custom adb path
    let adb_path = "adb";
    let runtime = RealRuntime::new(adb_path);

    let runtime_dir = dirs::runtime_dir()
        .or_else(|| dirs::cache_dir()).expect("missing cache dir")
        .join("adp");
    std::fs::create_dir_all(&runtime_dir)?;

    let sem = Semaphore::open("adp", 0)?;
    let app = App::new(runtime, runtime_dir, &sem);

    let resource = app.acquire_resource(std::process::id() as Pid)?;

    let mut cmd = Command::new(cmd);
    let cmd = cmd
        .env("ANDROID_SERIAL", &resource.serial)
        .args(args);

    info!(ANDROID_SERIAL = %resource.serial, cmd = ?cmd);

    let result = cmd.status();
    resource.release()?;
    result?.exit_ok_()?;

    Ok(())
}

#[derive(Debug, Delegate)]
#[delegate(Runtime, target = "runtime")]
pub struct App<'a, R: Runtime + Debug> {
    runtime: R,
    sem: &'a Semaphore,
    lock_file_path: PathBuf,
}

#[derive(Debug)]
pub struct Resource<'a, R: Runtime + Debug> {
    pub serial: String,
    app: &'a App<'a, R>,
    guard: SemaphoreGuard<'a>,
}

impl<R: Runtime + Debug> App<'_, R> {
    pub fn new(runtime: R, runtime_dir: impl AsRef<Path>, sem: &Semaphore) -> App<R> {
        let lock_file_path = runtime_dir.as_ref().join("adp.lock");
        App { runtime, sem, lock_file_path }
    }

    #[instrument]
    fn acquire_resource(&self, pid: Pid) -> Result<Resource<'_, R>> {
        loop {
            debug!("try_acquire_resource start");
            let resource = self.try_acquire_resource(pid)?;
            debug!("try_acquire_resource end");
            debug!(resource = ?resource);
            match resource {
                Some(resource) => {
                    resource.wait_for_ready()?;
                    return Ok(resource);
                }
                None => {
                    // try again
                }
            }
        }
    }

    #[instrument]
    fn try_acquire_resource(&self, pid: Pid) -> Result<Option<Resource<'_, R>>> {
        let serials = self.devices()?;
        debug!(serials = %serials.join(","));

        let mut lock_file = open_lock_file(&self.lock_file_path)?;
        debug!(lock_file = ?*lock_file);

        let mut entries = LockFileEntries::read(BufReader::new(&*lock_file))?;
        entries.update(&serials);

        let mut actual_value = entries.count_available();

        let mut serial = entries.acquire(pid);
        if serial.is_none() {
            // Check to see if any claimed serial is no longer running.
            let mut dropped = Vec::new();
            for (serial, pid) in entries.unavialble() {
                debug!(check = %serial);
                if !self.is_running(*pid)? {
                    dropped.push(serial.clone());
                }
            }
            entries.release_all(dropped);
            // and try again.
            actual_value = entries.count_available();
            serial = entries.acquire(pid);
        }

        debug!(serial = ?serial, entries = %entries);

        let value = self.sem.value()?;

        if value > actual_value {
            debug!(value = value, adjust_to = actual_value);
            for _ in actual_value..value {
                self.sem.acquire()?;
            }
            debug!(value = self.sem.value()?);
        } else if value < actual_value {
            debug!(value = value, adjust_to = actual_value);
            for _ in value..actual_value {
                self.sem.release()?;
            }
            debug!(value = self.sem.value()?);
        } else {
            debug!(value = value);
        }

        if serial.is_some() {
            lock_file.seek(SeekFrom::Start(0))?;
            lock_file.set_len(0)?;
            entries.write(BufWriter::new(&*lock_file))?;
        }

        // Ensure lock file is dropped before we block on the resource, to not deadlock with others
        // accessing it.
        drop(lock_file);
        let guard = self.sem.access()?;

        if let Some(serial) = serial {
            Ok(Some(Resource { serial, app: self, guard }))
        } else {
            Ok(None)
        }
    }
}

impl<R: Runtime + Debug> Resource<'_, R> {
    pub fn wait_for_ready(&self) -> Result<()> {
        self.app.wait_for_boot(&self.serial)?;
        Ok(())
    }

    #[instrument]
    pub fn release(self) -> Result<()> {
        let mut lock_file = open_lock_file(&self.app.lock_file_path)?;
        let mut entries = LockFileEntries::read(BufReader::new(&*lock_file))?;

        debug!(serial = %self.serial, entries = %entries);
        entries.release(self.serial.clone());
        debug!(serial = %self.serial, entries = %entries);

        lock_file.seek(SeekFrom::Start(0))?;
        lock_file.set_len(0)?;
        entries.write(BufWriter::new(&*lock_file))?;

        Ok(())
    }
}

fn open_lock_file(path: impl AsRef<Path>) -> Result<FileLockGuard> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path.as_ref()).with_context(|| format!("failed to open {:?}", path.as_ref()))?
        .into_lock_exclusive()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::RecvTimeoutError;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use ::function_name::named;
    use named_semaphore::Semaphore;
    use sysinfo::Pid;
    use temp_testdir::TempDir;
    use tracing::debug;
    use try_block::try_block;

    use crate::{App, debug_log};
    use crate::runtime::{Runtime, Serial};

    use super::Result;

    macro_rules! test_semaphore {
        () => {{
            let sem = Semaphore::open(function_name!(), 0)?;
            sem.unlink()?;
            sem
        }};
    }

    #[test]
    #[named]
    fn single_device_single_run_first_time() -> Result<()> {
        debug_log();
        let runtime = FakeRuntimeBuilder::default()
            .devices(vec!["serial1".to_string()])
            .build()?;
        let runtime_dir = TempDir::default();
        let sem = test_semaphore!();

        let app = App::new(runtime, &runtime_dir, &sem);
        let resource = app.acquire_resource(1)?;

        assert_eq!(resource.serial, "serial1");
        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial1:1\n");

        resource.release()?;

        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial1\n");

        Ok(())
    }

    #[test]
    #[named]
    fn single_device_single_run_second_time() -> Result<()> {
        debug_log();
        // let adb = FakeAdb(vec!["serial1".to_string()]);
        let runtime = FakeRuntimeBuilder::default()
            .devices(vec!["serial1".to_string()])
            .build()?;
        let runtime_dir = TempDir::default();
        std::fs::write(runtime_dir.join("adp.lock"), "serial1\n")?;

        let sem = test_semaphore!();
        let app = App::new(runtime, &runtime_dir, &sem);
        let resource = app.acquire_resource(1)?;

        assert_eq!(resource.serial, "serial1");
        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial1:1\n");
        assert_eq!(sem.value()?, 0);

        resource.release()?;

        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial1\n");
        assert_eq!(sem.value()?, 1);

        Ok(())
    }

    #[test]
    #[named]
    fn single_device_three_runs() -> Result<()> {
        debug_log();
        let runtime = FakeRuntimeBuilder::default()
            .devices(vec!["serial1".to_string()])
            .build()?;
        let runtime_dir = TempDir::default();
        let sem = test_semaphore!();
        let app = App::new(runtime, &runtime_dir, &sem);

        for _ in 0..3 {
            let resource = app.acquire_resource(1)?;
            resource.release()?;
        }

        Ok(())
    }

    #[test]
    #[named]
    fn multiple_devices_multiple_runs_first_time() -> Result<()> {
        debug_log();
        let runtime = FakeRuntimeBuilder::default()
            .devices(vec!["serial1".to_string(), "serial2".to_string()])
            .build()?;
        let runtime_dir = TempDir::default();

        let sem = test_semaphore!();
        let app = App::new(runtime, &runtime_dir, &sem);
        let resource1 = app.acquire_resource(1)?;
        let resource2 = app.acquire_resource(2)?;

        assert_eq!(resource1.serial, "serial1");
        assert_eq!(resource2.serial, "serial2");
        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial1:1\nserial2:2\n");
        assert_eq!(sem.value()?, 0);

        resource1.release()?;
        resource2.release()?;

        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial1\nserial2\n");
        assert_eq!(sem.value()?, 2);

        Ok(())
    }

    #[test]
    #[named]
    fn resource_blocks_until_one_is_released() -> Result<()> {
        debug_log();
        let runtime = FakeRuntimeBuilder::default()
            .devices(vec!["serial1".to_string()])
            .processes(vec![1])
            .build()?;
        let runtime_dir = TempDir::default();
        let sem_name = function_name!();
        let sem = Semaphore::open(sem_name, 0)?;
        let result: Result<JoinHandle<()>> = try_block! {
            let app = App::new(runtime.clone(), &runtime_dir, &sem);
            let resource1 = app.acquire_resource(1)?;

            let (send, recv) = std::sync::mpsc::channel();
            // This should block until resource1 is released.
            let handle = std::thread::spawn(move || {
                debug_log();
                let sem = Semaphore::open(sem_name, 0).unwrap();
                let app = App::new(runtime.clone(), &runtime_dir, &sem);
                let resource2 = app.acquire_resource(2).unwrap();
                let serial = resource2.serial.clone();
                debug!(send = %serial);
                send.send(serial).unwrap();
                resource2.release().unwrap();
            });

            let result = recv.recv_timeout(Duration::from_millis(500));
            match result {
                Ok(value) => {
                    panic!("expected to be blocked but got: {}", value);
                }
                Err(e) => {
                    assert_eq!(e, RecvTimeoutError::Timeout)
                }
            }

            resource1.release()?;
            debug!(sem = sem.value()?);

            let result = recv.recv_timeout(Duration::from_millis(500))?;
            assert_eq!(result, "serial1");

            Ok(handle)
        };
        sem.unlink()?;

        result?.join().expect("failed to join thread");

        Ok(())
    }

    #[test]
    #[named]
    fn obtains_the_correct_resource_when_device_is_removed() -> Result<()> {
        debug_log();
        let runtime = FakeRuntimeBuilder::default()
            .devices(vec!["serial2".to_string()])
            .build()?;
        let runtime_dir = TempDir::default();
        std::fs::write(runtime_dir.join("adp.lock"), "serial1\nserial2\n")?;

        let sem = test_semaphore!();
        let app = App::new(runtime, &runtime_dir, &sem);
        let resource = app.acquire_resource(1)?;

        assert_eq!(resource.serial, "serial2");
        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial2:1\n");
        assert_eq!(sem.value()?, 0);

        resource.release()?;

        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial2\n");
        assert_eq!(sem.value()?, 1);

        Ok(())
    }

    #[test]
    #[named]
    fn obtains_resource_if_process_is_no_longer_running() -> Result<()> {
        debug_log();
        let runtime = FakeRuntimeBuilder::default()
            .devices(vec!["serial1".to_string()])
            .build()?;
        let runtime_dir = TempDir::default();
        std::fs::write(runtime_dir.join("adp.lock"), "serial1:1\n")?;

        let sem = test_semaphore!();
        let app = App::new(runtime, &runtime_dir, &sem);
        let resource = app.acquire_resource(2)?;

        assert_eq!(resource.serial, "serial1");
        assert_eq!(std::fs::read_to_string(runtime_dir.join("adp.lock"))?, "serial1:2\n");

        Ok(())
    }

    #[derive(Debug, Clone, Default, Builder)]
    struct FakeRuntime {
        #[builder(default = "vec![]")]
        devices: Vec<Serial>,
        #[builder(default = "vec![]")]
        processes: Vec<Pid>,
    }

    impl Runtime for FakeRuntime {
        fn devices(&self) -> crate::runtime::Result<Vec<Serial>> {
            Ok(self.devices.clone())
        }

        fn wait_for_boot(&self, _serial: &Serial) -> crate::runtime::Result<()> {
            Ok(())
        }

        fn is_running(&self, pid: crate::runtime::Pid) -> crate::runtime::Result<bool> {
            Ok(self.processes.contains(&pid))
        }
    }
}
