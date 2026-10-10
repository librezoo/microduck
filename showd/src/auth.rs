//! Pairing and bearer tokens.
//!
//! **This is the debugging scheme Kivi already has, kept compatible, and it is labelled as such.**
//! A pairing code (eight characters, generated once and logged at start) is exchanged for a bearer
//! token over plain HTTP. That authenticates nobody against a hostile LAN; it stops the
//! accidental. The replacement — a button-confirmed, per-client credential issued by `btd` — is
//! stage K4 of `docs/ideas/kivi-show-integration.md`, and until it lands nothing here should be
//! described as secure.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the OS has randomness");
    b
}

fn hash(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairError {
    /// Too many wrong codes in a short time.
    Locked,
    Wrong,
}

pub struct Auth {
    dir: PathBuf,
    code: String,
    tokens: Mutex<Vec<String>>,
    static_token: Option<String>,
    failures: Mutex<VecDeque<Instant>>,
}

impl Auth {
    pub fn load(
        dir: &Path,
        fixed_code: Option<String>,
        static_token: Option<String>,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let code = match fixed_code {
            Some(c) => c,
            None => match std::fs::read_to_string(dir.join("pair-code")) {
                Ok(c) if c.trim().len() == 8 => c.trim().to_owned(),
                _ => {
                    let raw = random_bytes::<8>();
                    let c: String = raw
                        .iter()
                        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
                        .collect();
                    write_private(&dir.join("pair-code"), c.as_bytes())?;
                    c
                }
            },
        };
        let tokens: Vec<String> = std::fs::read_to_string(dir.join("tokens"))
            .map(|t| {
                t.lines()
                    .map(str::to_owned)
                    .filter(|l| l.len() == 64)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            dir: dir.to_owned(),
            code,
            tokens: Mutex::new(tokens),
            static_token,
            failures: Mutex::new(VecDeque::new()),
        })
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    /// Exchange the pairing code for a new token.
    pub fn pair(&self, code: &str) -> Result<String, PairError> {
        let now = Instant::now();
        {
            let mut f = self.failures.lock().expect("not poisoned");
            while f
                .front()
                .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
            {
                f.pop_front();
            }
            if f.len() >= 5 {
                return Err(PairError::Locked);
            }
        }
        if hash(&code.trim().to_uppercase()) != hash(&self.code.to_uppercase()) {
            self.failures.lock().expect("not poisoned").push_back(now);
            return Err(PairError::Wrong);
        }
        let token: String = random_bytes::<24>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut tokens = self.tokens.lock().expect("not poisoned");
        tokens.push(hash(&token));
        let _ = write_private(&self.dir.join("tokens"), tokens.join("\n").as_bytes());
        Ok(token)
    }

    pub fn check(&self, token: &str) -> bool {
        if self.static_token.as_deref() == Some(token) && !token.is_empty() {
            return true;
        }
        let h = hash(token);
        self.tokens.lock().expect("not poisoned").contains(&h)
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "showd-auth-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_code_buys_a_token_that_survives_a_restart_and_wrong_codes_lock_out() {
        let d = dir();
        let auth = Auth::load(&d, None, None).unwrap();
        let code = auth.code().to_owned();
        assert_eq!(code.len(), 8);
        let token = auth.pair(&code).unwrap();
        assert!(auth.check(&token) && !auth.check("nope"));
        let again = Auth::load(&d, None, None).unwrap();
        assert_eq!(again.code(), code, "the code is stable");
        assert!(again.check(&token), "and so is the token");
        for _ in 0..5 {
            assert_eq!(again.pair("WRONGCOD"), Err(PairError::Wrong));
        }
        assert_eq!(
            again.pair(&code),
            Err(PairError::Locked),
            "even the right one, for a minute"
        );
    }

    #[test]
    fn a_static_token_is_the_debugging_escape_and_empty_never_matches() {
        let auth = Auth::load(&dir(), Some("FIXEDCOD".into()), Some("debug-token".into())).unwrap();
        assert!(auth.check("debug-token") && !auth.check(""));
    }
}
