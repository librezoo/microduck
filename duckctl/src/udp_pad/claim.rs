//! Which pad is this instance's, when several `duckctl udp-pad` run on one laptop, one per duck.
//!
//! Every instance sees every pad, and nothing gilrs reports tells two identical pads apart across
//! processes — same name, same UUID, same vendor and product, and a `GamepadId` that is only an
//! index into this process's own list. So a pad is claimed by pressing a button on it, and the
//! instances agree through a directory on this machine:
//!
//! - **One claims at a time.** The instance waiting for a press holds `claiming.lock`; a second
//!   one waits its turn, so one press is never taken by two instances.
//! - **A claimed pad is skipped.** An instance with a pad writes the time of that pad's latest
//!   press to a file of its own. The same press reaches every instance at once, so a press the
//!   claiming instance sees at the time another instance wrote is a press on that one's pad.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How far apart two instances may stamp the same press. gilrs stamps an event when this
/// process reads it, a millisecond or so after the press; a human pressing two pads within this
/// window is the case it would get wrong, and pressing again settles it.
pub const SAME_PRESS: Duration = Duration::from_millis(100);

/// How long a press waits before it is taken: long enough for the owner of that pad, polling every
/// few milliseconds, to have written it down.
pub const SETTLE: Duration = Duration::from_millis(150);

pub struct Claims {
    dir: PathBuf,
    /// Held while this instance waits for a press.
    lock: Option<File>,
}

impl Claims {
    pub fn open() -> std::io::Result<Self> {
        Self::at(std::env::temp_dir().join("duckctl-udp-pad"))
    }

    fn at(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, lock: None })
    }

    /// Whether this instance may take the next press: true once it holds the lock, false while
    /// another instance does.
    pub fn turn(&mut self) -> std::io::Result<bool> {
        if self.lock.is_some() {
            return Ok(true);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("claiming.lock"))?;
        match file.try_lock() {
            Ok(()) => {
                self.lock = Some(file);
                Ok(true)
            }
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// Done claiming: the next instance may take a press.
    pub fn done(&mut self) {
        self.lock = None;
    }

    fn mine(&self) -> PathBuf {
        self.dir.join(format!("{}.pad", std::process::id()))
    }

    /// A press on this instance's pad, and the duck it drives.
    pub fn pressed(&self, at: SystemTime, to: &str) -> std::io::Result<()> {
        std::fs::write(self.mine(), format!("{} {to}", millis(at)))
    }

    /// The duck another instance drives with the pad pressed at `at`, if it is theirs.
    pub fn taken(&self, at: SystemTime) -> Option<String> {
        let at = millis(at);
        let mine = self.mine();
        std::fs::read_dir(&self.dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pad") && *p != mine)
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .find_map(|s| {
                let (t, to) = s.split_once(' ')?;
                let t: u128 = t.parse().ok()?;
                (t.abs_diff(at) <= SAME_PRESS.as_millis()).then(|| to.to_owned())
            })
    }
}

impl Drop for Claims {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(self.mine())
            && e.kind() != ErrorKind::NotFound
        {
            eprintln!("could not remove {}: {e}", self.mine().display());
        }
    }
}

fn millis(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_instance_claims_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Claims::at(dir.path().to_owned()).unwrap();
        let mut b = Claims::at(dir.path().to_owned()).unwrap();
        assert!(a.turn().unwrap());
        assert!(a.turn().unwrap(), "still its turn");
        assert!(!b.turn().unwrap(), "a is claiming");
        a.done();
        assert!(b.turn().unwrap());
    }

    /// Two instances in one process share a pid, so the other's file is written by hand here.
    #[test]
    fn a_press_on_another_instances_pad_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let claims = Claims::at(dir.path().to_owned()).unwrap();
        let t = UNIX_EPOCH + Duration::from_secs(1_000_000);
        std::fs::write(
            dir.path().join("1.pad"),
            format!("{} 192.168.1.42:4210", millis(t)),
        )
        .unwrap();
        assert_eq!(
            claims.taken(t + Duration::from_millis(3)).as_deref(),
            Some("192.168.1.42:4210")
        );
        assert_eq!(
            claims.taken(t - Duration::from_millis(3)).as_deref(),
            Some("192.168.1.42:4210")
        );
        assert_eq!(
            claims.taken(t + Duration::from_secs(1)),
            None,
            "another press"
        );

        // Its own pad's presses are not another instance's.
        claims
            .pressed(t + Duration::from_secs(5), "duck.local:4210")
            .unwrap();
        assert_eq!(claims.taken(t + Duration::from_secs(5)), None);
    }
}
