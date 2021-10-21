use core::option::Option;
use core::option::Option::{None, Some};
use core::result::Result::Ok;
use std::collections::BTreeMap;
use std::fmt::{Debug, Display, Formatter};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};

use tracing::{debug, instrument};

use crate::runtime::{Pid, Serial};

type Result<T> = std::io::Result<T>;

#[derive(Debug)]
pub struct LockFileEntries(BTreeMap<String, Option<Pid>>);

impl LockFileEntries {
    pub fn acquire(&mut self, pid: Pid) -> Option<Serial> {
        let serial = self.find_available()?;
        self.0.insert(serial.clone(), Some(pid));
        Some(serial)
    }

    fn find_available(&self) -> Option<Serial> {
        let (serial, _) = self.0.iter()
            .find(|(_, pid)| pid.is_none())?;
        Some(serial.to_string())
    }

    #[instrument]
    pub fn release(&mut self, serial: Serial) {
        debug!(release = %serial);
        self.0.insert(serial, None);
    }

    pub fn release_all(&mut self, serials: Vec<Serial>) {
        for serial in serials {
            self.release(serial);
        }
    }

    pub fn count_available(&self) -> usize {
        self.0.iter().filter(|(_, pid)| pid.is_none()).count()
    }

    pub fn unavialble(&self) -> impl Iterator<Item=(&Serial, &Pid)> {
        self.0.iter().filter_map(|(serial, pid)| {
            match pid {
                None => None,
                Some(pid) => Some((serial, pid))
            }
        })
    }

    #[instrument]
    pub fn update(&mut self, serials: &[Serial]) {
        // clean out disconnected
        self.0.retain(|serial, _| {
            debug!(remove = %serial);
            serials.contains(serial)
        });
        // add connected
        for serial in serials {
            self.0.entry(serial.to_string()).or_insert_with(|| {
                debug!(insert = %serial);
                None
            });
        }
    }

    #[instrument]
    pub fn read<R: Read + Debug>(reader: R) -> Result<LockFileEntries> {
        let reader = BufReader::new(reader);
        let entries: BTreeMap<_, _> = reader.lines()
            .map(|line| line.map(|line| {
                let mut parts = line.split(":");
                let entry = (
                    parts.next().unwrap().to_string(),
                    parts.next().map(|s| s.to_string().parse().expect("invalid pid")),
                );
                entry
            }))
            .collect::<std::io::Result<_>>()?;
        let entries = LockFileEntries(entries);
        debug!(entries = %entries);
        Ok(entries)
    }

    #[instrument]
    pub fn write<W: Write + Debug>(&self, writer: W) -> Result<()> {
        let mut writer = BufWriter::new(writer);
        for (serial, pid) in &self.0 {
            debug!(serial = ?serial, pid = ?pid);
            write!(writer, "{}", serial)?;
            match &pid {
                Some(pid) => {
                    write!(writer, ":{}", pid)?;
                }
                None => {}
            }
            write!(writer, "\n")?;
        }
        Ok(())
    }
}

impl Display for LockFileEntries {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for (i, (serial, pid)) in self.0.iter().enumerate() {
            if i != 0 {
                write!(f, ",")?;
            }
            write!(f, "{}", serial)?;
            match &pid {
                Some(pid) => {
                    write!(f, ":{}", pid)?;
                }
                None => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Result, Write};

    use crate::lockfile::LockFileEntries;

    #[test]
    fn reads_entries() -> Result<()> {
        let input = "serial1\nserial2:2\nserial3\n";
        let entries = LockFileEntries::read(input.as_bytes())?;

        assert_eq!(format!("{}", entries), "serial1,serial2:2,serial3");

        Ok(())
    }

    #[test]
    fn writes_entries() -> Result<()> {
        let input = "serial1\nserial2:2\nserial3\n";
        let entries = LockFileEntries::read(input.as_bytes())?;
        let mut output = Vec::new();
        entries.write(Cursor::new(&mut output))?;

        assert_eq!(String::from_utf8(output).unwrap(), "serial1\nserial2:2\nserial3\n");

        Ok(())
    }

    #[test]
    fn inserts_new_entries() -> Result<()> {
        let input = "serial1\nserial2:2\n";
        let mut entries = LockFileEntries::read(input.as_bytes())?;
        entries.update(&["serial1".to_string(), "serial2".to_string(), "serial3".to_string()]);

        assert_eq!(format!("{}", entries), "serial1,serial2:2,serial3");

        Ok(())
    }

    #[test]
    fn removes_old_entries() -> Result<()> {
        let input = "serial1\nserial2:2\n";
        let mut entries = LockFileEntries::read(input.as_bytes())?;
        entries.update(&["serial2".to_string()]);

        assert_eq!(format!("{}", entries), "serial2:2");

        Ok(())
    }

    #[test]
    fn acquires_entry_some() -> Result<()> {
        let input = "serial1\nserial2:2\n";
        let mut entries = LockFileEntries::read(input.as_bytes())?;
        let serial = entries.acquire(1);

        assert_eq!(serial, Some("serial1".to_string()));
        assert_eq!(format!("{}", entries), "serial1:1,serial2:2");

        Ok(())
    }

    #[test]
    fn acquires_entry_none() -> Result<()> {
        let input = "serial1:1\nserial2:2\n";
        let mut entries = LockFileEntries::read(input.as_bytes())?;
        let serial = entries.acquire(1);

        assert_eq!(serial, None);

        Ok(())
    }
}
