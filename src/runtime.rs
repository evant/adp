use std::cell::RefCell;
use std::path::Path;
use std::time::Duration;

use ambassador::delegatable_trait;
use anyhow::{anyhow, Context};
use sysinfo::{System, SystemExt};
use tracing::{debug, instrument};

use crate::adb::Adb;

pub type Result<T> = std::result::Result<T, anyhow::Error>;

pub type Serial = String;
pub type Pid = sysinfo::Pid;

#[delegatable_trait]
pub trait Runtime {
    fn devices(&self) -> Result<Vec<Serial>>;
    fn wait_for_boot(&self, serial: &Serial) -> Result<()>;
    fn is_running(&self, pid: Pid) -> Result<bool>;
}

#[derive(Debug)]
pub struct RealRuntime {
    adb: Adb,
    sys: RefCell<System>,
}

impl RealRuntime {
    pub fn new(adb_path: impl AsRef<Path>) -> RealRuntime {
        RealRuntime {
            adb: Adb::new(adb_path),
            sys: RefCell::new(System::new()),
        }
    }
}

impl Runtime for RealRuntime {
    fn devices(&self) -> Result<Vec<Serial>> {
        let mut devices = self.adb.devices()?;

        if devices.is_empty() {
            // wait for a device and try again
            self.adb.wait_for_device()?;
            devices = self.adb.devices()?;
        }

        Ok(devices)
    }

    #[instrument]
    fn wait_for_boot(&self, serial: &Serial) -> Result<()> {
        for (prop, expected_value) in [
            ("init.svc.bootanim", "stopped"),
            ("sys.boot_completed", "1"),
        ] {
            retry::<_, _, _, anyhow::Error, _>(
                retry::delay::Fixed::from(Duration::from_secs(1)).take(60),
                || {
                    debug!("reading prop {}", prop);
                    let value = self.adb.shell_getprop(serial, prop)?;
                    debug!(prop = %prop, value = %value);
                    if value != expected_value {
                        Err(anyhow!(
                            "expected prop {} = {} but was {}",
                            prop,
                            expected_value,
                            value
                        ))?;
                    }
                    Ok(())
                },
            )
            .with_context(|| format!("timed out waiting for prop {}", prop))?;
        }

        Ok(())
    }

    fn is_running(&self, pid: Pid) -> Result<bool> {
        // There doesn't seem to be a way to tell if this failed?
        Ok(self.sys.borrow_mut().refresh_process(pid))
    }
}

// Wrapper to not have to unwrap internal error
// https://github.com/jimmycuadra/retry/issues/38
fn retry<I, O, R, E, OR>(iterable: I, mut operation: O) -> std::result::Result<R, E>
where
    I: IntoIterator<Item = Duration>,
    O: FnMut() -> OR,
    OR: Into<retry::OperationResult<R, E>>,
{
    match retry::retry_with_index(iterable, |_| operation()) {
        Ok(value) => Ok(value),
        Err(e) => match e {
            retry::Error::Operation { error, .. } => Err(error),
            retry::Error::Internal(_) => unreachable!(),
        },
    }
}
