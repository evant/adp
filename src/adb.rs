use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::exitstatus::ExitStatusExt;

pub type Result<T> = std::result::Result<T, anyhow::Error>;

#[derive(Debug)]
pub struct Adb {
    path: PathBuf,
}

impl Adb {
    pub fn new(path: impl AsRef<Path>) -> Adb {
        return Adb {
            path: path.as_ref().to_path_buf()
        };
    }

    pub fn wait_for_device(&self) -> Result<()> {
        Command::new(&self.path)
            .arg("wait-for-device")
            .status()?
            .exit_ok_()?;
        Ok(())
    }

    pub fn shell_getprop(&self, serial: &str, name: &str) -> Result<String> {
        let output = Command::new(&self.path)
            .args(&["-s", serial, "shell", "getprop", name])
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        output.status.exit_ok_()?;

        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }

    pub fn devices(&self) -> Result<Vec<String>> {
        let output = Command::new(&self.path)
            .arg("devices")
            .arg("-l")
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        output.status.exit_ok_()?;

        let devices: Vec<_> = output.stdout.lines().skip(1)
            .map(|line| line.unwrap())
            .filter(|line| line.len() > 0)
            .map(|line| line.split_ascii_whitespace().next().unwrap().to_owned())
            .collect();

        Ok(devices)
    }
}