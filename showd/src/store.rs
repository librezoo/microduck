//! The show library on disk.
//!
//! ```text
//! <data>/shows/<id>/            a published show: manifest.json, its tracks, meta.json
//! <data>/staging/<id>/          tracks and a manifest on their way in
//! ```
//!
//! **Nothing is visible until it is whole.** Tracks arrive one at a time into staging, each
//! hashed as it is written; `commit` checks every track against the manifest, validates it, and
//! only then swaps the directory in with a rename. A transfer that stops halfway leaves staging
//! dirty and the library exactly as it was — which is what lets the app say "stored" only when it
//! is, and re-send only the tracks that changed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use show::{
    Capabilities, DuckScore, Manifest, MotionTrack, Pose, Problem, Problems, manifest::is_safe_id,
    sha256_hex,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub revision: String,
    pub updated_at_ms: u64,
    pub total_bytes: u64,
}

/// A published show, as the library lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub manifest: Manifest,
    pub meta: Meta,
    pub dir: PathBuf,
}

impl Entry {
    pub fn audio_path(&self) -> Option<PathBuf> {
        self.manifest
            .tracks
            .audio
            .as_ref()
            .map(|_| self.dir.join(AUDIO_FILE))
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// SHA-256 and length of a file, read in blocks so a 44 MB WAV costs 64 KiB, not 44 MB.
fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        hasher.update(&buf[..k]);
        n += k as u64;
    }
    Ok((
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        n,
    ))
}

fn problem(code: &'static str, detail: impl Into<String>) -> Problems {
    let mut p = Problems::default();
    p.push(Problem::new(code, detail));
    p
}

/// The one on-disk name of the audio track: the store owns it, whatever the manifest's `file` says.
pub const AUDIO_FILE: &str = "audio.wav";

/// Makes every upload's scratch file its own, so two uploads of one track never share an inode.
static PART_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Removes a scratch file on every way out (error, interrupted upload, dropped request) unless the
/// upload was renamed into place.
struct PartGuard(PathBuf);

impl Drop for PartGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const TRACK_FILES: [(&str, &str); 4] = [
    ("duck", "duck.score.json"),
    ("motion", "motion.json"),
    ("audio", AUDIO_FILE),
    ("expression", "expression.patch.json"),
];

impl Store {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root.join("shows"))?;
        std::fs::create_dir_all(root.join("staging"))?;
        Ok(Self {
            root: root.to_owned(),
        })
    }

    /// Where published shows live — the directory `robotd` may read audio from.
    pub fn shows_dir(&self) -> PathBuf {
        self.root.join("shows")
    }

    fn staging(&self, id: &str) -> PathBuf {
        self.root.join("staging").join(id)
    }

    fn published(&self, id: &str) -> PathBuf {
        self.root.join("shows").join(id)
    }

    pub fn list(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.root.join("shows")) else {
            return out;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".new") || name.ends_with(".old") {
                continue;
            }
            if let Some(entry) = self.get(&name) {
                out.push(entry);
            }
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.meta.updated_at_ms));
        out
    }

    pub fn get(&self, id: &str) -> Option<Entry> {
        if !is_safe_id(id) {
            return None;
        }
        let dir = self.published(id);
        let manifest: Manifest =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).ok()?).ok()?;
        let meta: Meta =
            serde_json::from_slice(&std::fs::read(dir.join("meta.json")).ok()?).ok()?;
        Some(Entry {
            manifest,
            meta,
            dir,
        })
    }

    pub fn delete(&self, id: &str) -> bool {
        if !is_safe_id(id) {
            return false;
        }
        let _ = std::fs::remove_dir_all(self.staging(id));
        std::fs::remove_dir_all(self.published(id)).is_ok()
    }

    /// Write one staged track, hashing as it goes. `chunks` are the body's pieces in order.
    /// Returns `(sha256, bytes)`; a body bigger than `limit` is abandoned.
    pub async fn put_track<S, B, E>(
        &self,
        id: &str,
        name: &str,
        mut body: S,
        limit: u64,
    ) -> Result<(String, u64), Problems>
    where
        S: futures_util::Stream<Item = Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        if !is_safe_id(id) {
            return Err(problem("Corrupt", "bad show id"));
        }
        let Some((_, file)) =
            TRACK_FILES
                .iter()
                .find(|(n, _)| *n == name)
                .copied()
                .or(if name == "manifest" {
                    Some(("manifest", "manifest.json"))
                } else {
                    None
                })
        else {
            return Err(problem("Corrupt", format!("no track called {name:?}")));
        };
        let dir = self.staging(id);
        std::fs::create_dir_all(&dir).map_err(|e| problem("Io", e.to_string()))?;
        let part = dir.join(format!(
            "{file}.{}.part",
            PART_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _guard = PartGuard(part.clone());
        let mut out = tokio::fs::File::create(&part)
            .await
            .map_err(|e| problem("Io", e.to_string()))?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        while let Some(chunk) = body.next().await {
            let chunk =
                chunk.map_err(|e| problem("Corrupt", format!("upload interrupted: {e}")))?;
            let bytes = chunk.as_ref();
            total += bytes.len() as u64;
            if total > limit {
                return Err(problem("TooLarge", format!("more than {limit} bytes")));
            }
            hasher.update(bytes);
            out.write_all(bytes)
                .await
                .map_err(|e| problem("Io", e.to_string()))?;
        }
        out.flush()
            .await
            .map_err(|e| problem("Io", e.to_string()))?;
        drop(out);
        std::fs::rename(&part, dir.join(file)).map_err(|e| problem("Io", e.to_string()))?;
        let sha: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok((sha, total))
    }

    /// Check what is staged against its manifest and, if it is all there and all valid, publish it.
    ///
    /// A track the manifest names but staging lacks is taken from the *published* copy when that
    /// copy has the same hash — so an edit to the motion track does not resend the audio.
    pub fn commit(
        &self,
        id: &str,
        caps: &Capabilities,
        actual_pose: Pose,
    ) -> Result<Entry, Problems> {
        if !is_safe_id(id) {
            return Err(problem("Corrupt", "bad show id"));
        }
        let staging = self.staging(id);
        let manifest_bytes = std::fs::read(staging.join("manifest.json"))
            .map_err(|_| problem("Missing", "no manifest was uploaded"))?;
        let manifest = Manifest::parse(&manifest_bytes)?;
        if manifest.show_id != id {
            return Err(problem(
                "Corrupt",
                "the manifest's showId is not the id it was uploaded under",
            ));
        }
        let previous = self.get(id);
        let mut p = Problems::default();
        let mut total = manifest_bytes.len() as u64;
        // Locate every track: the staged copy if it matches the manifest, else the published copy
        // if that matches. A stale staged track left by a refused commit must not hide a published
        // copy the manifest asks for (a revert sends nothing but the manifest).
        let mut sources: Vec<(&str, PathBuf)> = Vec::new();
        for (name, sha, bytes) in manifest.track_hashes() {
            let file = TRACK_FILES
                .iter()
                .find(|(n, _)| *n == name)
                .expect("known track")
                .1;
            let (mut seen, mut failed) = (false, false);
            let mut chosen = None;
            for dir in [Some(&staging), previous.as_ref().map(|e| &e.dir)]
                .into_iter()
                .flatten()
            {
                let path = dir.join(file);
                match sha256_file(&path) {
                    Ok((h, n)) if h == sha && n == bytes => {
                        chosen = Some(path);
                        break;
                    }
                    Ok(_) => seen = true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        failed = true;
                        p.push(Problem::new("Io", e.to_string()));
                    }
                }
            }
            let Some(path) = chosen else {
                if seen {
                    p.push(Problem::new(
                        "Corrupt",
                        format!("track {name}: hash or size does not match the manifest"),
                    ));
                } else if !failed {
                    p.push(Problem::new(
                        "Missing",
                        format!("track {name} was not uploaded"),
                    ));
                }
                continue;
            };
            if bytes > caps.max_show_bytes {
                p.push(Problem::new(
                    "TooLarge",
                    format!("track {name} is {bytes} bytes"),
                ));
            }
            total += bytes;
            sources.push((name, path));
        }
        p.0.is_empty().then_some(()).ok_or(p.clone())?;
        if total > caps.max_show_bytes {
            return Err(problem(
                "TooLarge",
                format!(
                    "{total} bytes in all; this robot takes {}",
                    caps.max_show_bytes
                ),
            ));
        }
        // Validate the contents.
        let mut problems = Problems::default();
        if let Some((_, path)) = sources.iter().find(|(n, _)| *n == "duck") {
            match DuckScore::parse(&std::fs::read(path).unwrap_or_default()).and_then(|s| {
                s.validate(caps.max_events)?;
                s.to_performance(&manifest.title).map(|_| s)
            }) {
                Ok(_) => {}
                Err(e) => problems.0.extend(e.0),
            }
        }
        if let Some((_, path)) = sources.iter().find(|(n, _)| *n == "motion") {
            let start = manifest
                .start_pose
                .as_deref()
                .map(Pose::from_name)
                .unwrap_or(Pose::Standing);
            match MotionTrack::parse(&std::fs::read(path).unwrap_or_default())
                .and_then(|m| m.validate(caps, start))
            {
                Ok(()) => {}
                Err(e) => problems.0.extend(e.0),
            }
        }
        if let (Some((_, path)), Some(a)) = (
            sources.iter().find(|(n, _)| *n == "audio"),
            &manifest.tracks.audio,
        ) {
            match hound::WavReader::open(path) {
                Ok(r) => {
                    let spec = r.spec();
                    if spec.sample_rate != a.sample_rate
                        || u8::try_from(spec.channels).ok() != Some(a.channels)
                        || spec.bits_per_sample != 16
                        || spec.sample_format != hound::SampleFormat::Int
                    {
                        problems.push(Problem::new(
                            "Corrupt",
                            "the audio file's format is not what the manifest says",
                        ));
                    }
                }
                Err(e) => problems.push(Problem::new(
                    "Corrupt",
                    format!("audio is not a readable WAV: {e}"),
                )),
            }
        }
        let _ = actual_pose;
        problems.into_result()?;

        // Assemble beside the old one and swap.
        let new = self.root.join("shows").join(format!("{id}.new"));
        let _ = std::fs::remove_dir_all(&new);
        std::fs::create_dir_all(&new).map_err(|e| problem("Io", e.to_string()))?;
        let assembled = (|| -> Result<Meta, Problems> {
            for (name, path) in &sources {
                let file = TRACK_FILES
                    .iter()
                    .find(|(n, _)| n == name)
                    .expect("known")
                    .1;
                let dest = new.join(file);
                if std::fs::hard_link(path, &dest).is_err() {
                    std::fs::copy(path, &dest).map_err(|e| problem("Io", e.to_string()))?;
                }
            }
            std::fs::write(new.join("manifest.json"), &manifest_bytes)
                .map_err(|e| problem("Io", e.to_string()))?;
            let meta = Meta {
                revision: sha256_hex(&manifest_bytes),
                updated_at_ms: now_ms(),
                total_bytes: total,
            };
            std::fs::write(
                new.join("meta.json"),
                serde_json::to_vec(&meta).expect("serialises"),
            )
            .map_err(|e| problem("Io", e.to_string()))?;
            Ok(meta)
        })();
        let meta = match assembled {
            Ok(m) => m,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&new);
                return Err(e);
            }
        };
        let live = self.published(id);
        let old = self.root.join("shows").join(format!("{id}.old"));
        let _ = std::fs::remove_dir_all(&old);
        if live.exists() {
            std::fs::rename(&live, &old).map_err(|e| problem("Io", e.to_string()))?;
        }
        if let Err(e) = std::fs::rename(&new, &live) {
            // Put the old one back rather than leave the library without it.
            let _ = std::fs::rename(&old, &live);
            return Err(problem("Io", e.to_string()));
        }
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&staging);
        Ok(Entry {
            manifest,
            meta,
            dir: live,
        })
    }

    /// Remove half-finished uploads older than `max_age_ms`. Safe to run while the service is up.
    pub fn sweep_staging(&self, max_age_ms: u64) {
        let Ok(rd) = std::fs::read_dir(self.root.join("staging")) else {
            return;
        };
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_millis() as u64 > max_age_ms);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    /// A restart's housekeeping: stale staging, and swaps that were interrupted. Only at start,
    /// when no commit can be in flight.
    pub fn sweep(&self, max_age_ms: u64) {
        self.sweep_staging(max_age_ms);
        // Interrupted swaps.
        if let Ok(rd) = std::fs::read_dir(self.root.join("shows")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with(".new") {
                    let _ = std::fs::remove_dir_all(e.path());
                } else if let Some(id) = name.strip_suffix(".old") {
                    // The swap died between its two renames: the old copy is the good one.
                    if !self.published(id).exists() {
                        let _ = std::fs::rename(e.path(), self.published(id));
                    } else {
                        let _ = std::fs::remove_dir_all(e.path());
                    }
                }
            }
        }
    }
}
