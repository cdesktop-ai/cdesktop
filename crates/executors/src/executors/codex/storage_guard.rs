use std::{
    env, io,
    path::{Path, PathBuf},
};

use super::codex_home;

const DEFAULT_MAX_ROLLOUT_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const MAX_ROLLOUT_ENV: &str = "CDESKTOP_MAX_CODEX_ROLLOUT_BYTES";
const MIN_FREE_ENV: &str = "CDESKTOP_MIN_FREE_DISK_BYTES";

#[derive(Debug, Clone, Copy)]
pub(super) struct StorageLimits {
    max_rollout_bytes: u64,
    min_free_bytes: u64,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            max_rollout_bytes: env_limit(MAX_ROLLOUT_ENV, DEFAULT_MAX_ROLLOUT_BYTES),
            min_free_bytes: env_limit(MIN_FREE_ENV, DEFAULT_MIN_FREE_BYTES),
        }
    }
}

impl StorageLimits {
    pub(super) async fn ensure_start_allowed(&self) -> io::Result<()> {
        let home = codex_home().ok_or_else(|| io::Error::other("Codex home is unavailable"))?;
        let probe = existing_ancestor(&home);
        let available = fs2::available_space(&probe)?;
        if available < self.min_free_bytes {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                format!(
                    "Codex start refused: {available} free bytes is below the {} byte reserve",
                    self.min_free_bytes
                ),
            ));
        }
        Ok(())
    }

    pub(super) async fn ensure_fork_allowed(&self, thread_id: &str) -> io::Result<()> {
        self.ensure_start_allowed().await?;
        let path = find_rollout_file(thread_id).await.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Codex fork refused: source rollout {thread_id} was not found"),
            )
        })?;
        self.ensure_rollout_allowed(&path).await
    }

    pub(super) async fn ensure_rollout_allowed(&self, path: &Path) -> io::Result<()> {
        let bytes = tokio::fs::metadata(path).await?.len();
        if bytes > self.max_rollout_bytes {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!(
                    "Codex rollout stopped at {bytes} bytes; limit is {} bytes",
                    self.max_rollout_bytes
                ),
            ));
        }
        self.ensure_start_allowed().await
    }
}

pub(super) async fn find_rollout_file(thread_id: &str) -> Option<PathBuf> {
    let sessions = codex_home()?.join("sessions");
    find_in(&sessions, thread_id).await
}

async fn find_in(dir: &Path, thread_id: &str) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = Box::pin(find_in(&path, thread_id)).await {
                return Some(found);
            }
        } else if let Some(name) = path.file_name().and_then(|name| name.to_str())
            && name.starts_with("rollout-")
            && name.contains(thread_id)
            && name.ends_with(".jsonl")
        {
            return Some(path);
        }
    }
    None
}

fn existing_ancestor(path: &Path) -> PathBuf {
    let mut candidate = path.to_path_buf();
    while !candidate.exists() {
        if !candidate.pop() {
            return PathBuf::from(".");
        }
    }
    candidate
}

fn env_limit(name: &str, fallback: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oversized_rollout_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-test.jsonl");
        tokio::fs::write(&path, b"too large").await.unwrap();
        let limits = StorageLimits {
            max_rollout_bytes: 4,
            min_free_bytes: 1,
        };

        let error = limits.ensure_rollout_allowed(&path).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    }

    #[tokio::test]
    async fn disk_reserve_is_checked_before_launch() {
        let limits = StorageLimits {
            max_rollout_bytes: u64::MAX,
            min_free_bytes: u64::MAX,
        };

        let error = limits.ensure_start_allowed().await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
    }
}
