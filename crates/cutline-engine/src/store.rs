//! Where Cutline keeps things between runs: user profiles, the last
//! session, and caches. All plain JSON in the XDG directories.

use cutline_core::{Profile, Project};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn app_dir(base: Option<PathBuf>) -> PathBuf {
    base.unwrap_or_else(std::env::temp_dir).join("cutline")
}

pub fn config_dir() -> PathBuf {
    app_dir(dirs::config_dir())
}
pub fn state_dir() -> PathBuf {
    app_dir(dirs::state_dir().or_else(dirs::data_local_dir))
}
pub fn cache_dir() -> PathBuf {
    app_dir(dirs::cache_dir())
}

/// Write via a temp file and rename, so a crash never leaves half a file.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Session {
    pub project: Project,
    /// Playhead, as (clip id, seconds into that clip).
    #[serde(default)]
    pub playhead: Option<(u64, f64)>,
    /// Last folder exported to.
    #[serde(default)]
    pub last_export_dir: Option<PathBuf>,
}

pub fn session_path() -> PathBuf {
    state_dir().join("session.json")
}

pub fn load_session() -> Option<Session> {
    let bytes = std::fs::read(session_path()).ok()?;
    let mut s: Session = serde_json::from_slice(&bytes).ok()?;
    // Drop videos that have gone away since.
    let gone: Vec<u64> = s.project.clips.iter().filter(|c| !c.path.exists()).map(|c| c.id).collect();
    for id in gone {
        s.project.remove_clip(id);
    }
    s.project.normalise();
    Some(s)
}

pub fn save_session(s: &Session) -> std::io::Result<()> {
    write_atomic(&session_path(), &serde_json::to_vec_pretty(s).unwrap_or_default())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProfileStore {
    /// Profiles the user made in the Advanced editor.
    pub user: Vec<Profile>,
}

impl ProfileStore {
    pub fn path() -> PathBuf {
        config_dir().join("profiles.json")
    }
    pub fn load() -> ProfileStore {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self) -> std::io::Result<()> {
        write_atomic(&Self::path(), &serde_json::to_vec_pretty(self).unwrap_or_default())
    }
    /// Built-in presets followed by the user's own.
    pub fn presets(&self) -> Vec<Profile> {
        let mut v = Profile::presets();
        v.extend(self.user.iter().cloned());
        v
    }
    pub fn upsert(&mut self, p: Profile) {
        match self.user.iter_mut().find(|q| q.id == p.id) {
            Some(q) => *q = p,
            None => self.user.push(p),
        }
    }
    pub fn remove(&mut self, id: &str) {
        self.user.retain(|p| p.id != id);
    }
    pub fn new_id(&self) -> String {
        (1..).map(|i| format!("user-{i}")).find(|id| !self.user.iter().any(|p| &p.id == id)).unwrap()
    }
}
