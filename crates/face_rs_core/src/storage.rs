use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::{FaceRsError, Result};
use crate::matching::Embedding;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrolledTemplate {
    pub id: u32,
    pub label: String,
    pub created_at: String,
    pub embedding: Embedding,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserTemplates {
    pub templates: Vec<EnrolledTemplate>,
}

pub struct TemplateStore {
    base_dir: PathBuf,
}

impl TemplateStore {
    pub fn new(base_dir: &Path) -> Self {
        TemplateStore {
            base_dir: base_dir.to_owned(),
        }
    }

    fn validate_username(username: &str) -> Result<()> {
        if username.is_empty() || username.len() > 64 {
            return Err(FaceRsError::Storage(
                "username must be 1-64 characters".to_string(),
            ));
        }
        if username == "." || username == ".." {
            return Err(FaceRsError::Storage("invalid username".to_string()));
        }
        if !username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(FaceRsError::Storage(format!(
                "invalid username '{username}': only ASCII letters, digits, '.', '_' and '-' are allowed"
            )));
        }
        Ok(())
    }

    fn user_dir(&self, username: &str) -> Result<PathBuf> {
        Self::validate_username(username)?;
        Ok(self.base_dir.join(username))
    }

    fn embeddings_path(&self, username: &str) -> Result<PathBuf> {
        Ok(self.user_dir(username)?.join("embeddings.json"))
    }

    fn ensure_base_dir(&self) -> Result<()> {
        if !self.base_dir.exists() {
            fs::create_dir_all(&self.base_dir).map_err(|e| {
                FaceRsError::Storage(format!("cannot create {}: {e}", self.base_dir.display()))
            })?;
            fs::set_permissions(&self.base_dir, fs::Permissions::from_mode(0o755)).map_err(
                |e| {
                    FaceRsError::Storage(format!(
                        "cannot set permissions on {}: {e}",
                        self.base_dir.display()
                    ))
                },
            )?;
        }

        let meta = fs::symlink_metadata(&self.base_dir).map_err(|e| {
            FaceRsError::Storage(format!("cannot inspect {}: {e}", self.base_dir.display()))
        })?;
        if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
            return Err(FaceRsError::Storage(format!(
                "{} is not a real directory",
                self.base_dir.display()
            )));
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o022 != 0 {
            return Err(FaceRsError::Storage(format!(
                "{} has unsafe permissions {:o}; expected no group/world write",
                self.base_dir.display(),
                mode
            )));
        }
        Ok(())
    }

    fn ensure_user_dir(&self, username: &str) -> Result<PathBuf> {
        self.ensure_base_dir()?;
        let dir = self.user_dir(username)?;
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| {
                FaceRsError::Storage(format!("cannot create {}: {e}", dir.display()))
            })?;
        }

        let meta = fs::symlink_metadata(&dir)
            .map_err(|e| FaceRsError::Storage(format!("cannot inspect {}: {e}", dir.display())))?;
        if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
            return Err(FaceRsError::Storage(format!(
                "{} is not a real directory",
                dir.display()
            )));
        }

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| {
            FaceRsError::Storage(format!("cannot set permissions on {}: {e}", dir.display()))
        })?;
        Ok(dir)
    }

    fn validate_existing_file(path: &Path) -> Result<()> {
        let meta = fs::symlink_metadata(path)
            .map_err(|e| FaceRsError::Storage(format!("cannot inspect {}: {e}", path.display())))?;
        if !meta.file_type().is_file() || meta.file_type().is_symlink() {
            return Err(FaceRsError::Storage(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(FaceRsError::Storage(format!(
                "{} has unsafe permissions {:o}; expected 0600",
                path.display(),
                mode
            )));
        }
        Ok(())
    }

    fn write_json_atomic(path: &Path, json: &[u8]) -> Result<()> {
        let dir = path.parent().ok_or_else(|| {
            FaceRsError::Storage(format!("{} has no parent directory", path.display()))
        })?;
        let file_name = path
            .file_name()
            .ok_or_else(|| FaceRsError::Storage(format!("{} has no file name", path.display())))?;
        let tmp_path = dir.join(format!(
            ".{}.{}.tmp",
            file_name.to_string_lossy(),
            std::process::id()
        ));

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp_path)
            .map_err(|e| {
                FaceRsError::Storage(format!("cannot create {}: {e}", tmp_path.display()))
            })?;

        let write_result = (|| -> Result<()> {
            file.write_all(json).map_err(|e| {
                FaceRsError::Storage(format!("cannot write {}: {e}", tmp_path.display()))
            })?;
            file.sync_all().map_err(|e| {
                FaceRsError::Storage(format!("cannot sync {}: {e}", tmp_path.display()))
            })?;
            fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600)).map_err(|e| {
                FaceRsError::Storage(format!(
                    "cannot set permissions on {}: {e}",
                    tmp_path.display()
                ))
            })?;
            Ok(())
        })();

        if let Err(e) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }

        fs::rename(&tmp_path, path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            FaceRsError::Storage(format!(
                "cannot replace {} with {}: {e}",
                path.display(),
                tmp_path.display()
            ))
        })?;
        Ok(())
    }

    #[allow(dead_code)]
    fn user_dir_unchecked_for_tests(&self, username: &str) -> PathBuf {
        self.base_dir.join(username)
    }

    pub fn load(&self, username: &str) -> Result<UserTemplates> {
        self.ensure_base_dir()?;
        let path = self.embeddings_path(username)?;
        if !path.exists() {
            return Ok(UserTemplates::default());
        }
        Self::validate_existing_file(&path)?;
        let content = fs::read_to_string(&path)?;
        let templates = serde_json::from_str(&content)?;
        Ok(templates)
    }

    pub fn save(&self, username: &str, templates: &UserTemplates) -> Result<()> {
        let _dir = self.ensure_user_dir(username)?;
        let path = self.embeddings_path(username)?;
        let json = serde_json::to_string_pretty(templates)?;
        Self::write_json_atomic(&path, json.as_bytes())?;
        Ok(())
    }

    pub fn add_template(
        &self,
        username: &str,
        label: &str,
        embedding: Embedding,
    ) -> Result<EnrolledTemplate> {
        let mut store = self.load(username)?;
        let id = store.templates.len() as u32;
        let created_at = chrono_now();
        let template = EnrolledTemplate {
            id,
            label: label.to_owned(),
            created_at,
            embedding,
        };
        store.templates.push(template.clone());
        self.save(username, &store)?;
        Ok(template)
    }

    pub fn remove_template(&self, username: &str, id: u32) -> Result<()> {
        let mut store = self.load(username)?;
        let before = store.templates.len();
        store.templates.retain(|t| t.id != id);
        if store.templates.len() == before {
            return Err(FaceRsError::Storage(format!(
                "no template with id {id} for user {username}"
            )));
        }
        // Re-number IDs to stay contiguous
        for (i, t) in store.templates.iter_mut().enumerate() {
            t.id = i as u32;
        }
        self.save(username, &store)
    }

    pub fn embeddings_for(&self, username: &str) -> Result<Vec<Embedding>> {
        let store = self.load(username)?;
        if store.templates.is_empty() {
            return Err(FaceRsError::NotEnrolled);
        }
        Ok(store.templates.into_iter().map(|t| t.embedding).collect())
    }
}

fn chrono_now() -> String {
    // Poor-man's timestamp without pulling in chrono — good enough for MVP.
    // Replace with chrono or time crate when needed.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Emit as Unix timestamp for now; format nicely once we add chrono.
    secs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, TemplateStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = TemplateStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn load_missing_user_returns_empty() {
        let (_dir, store) = temp_store();
        let templates = store.load("nobody").expect("load");
        assert!(templates.templates.is_empty());
    }

    #[test]
    fn add_and_reload() {
        let (_dir, store) = temp_store();
        let emb = vec![0.1_f32, 0.2, 0.3];
        store
            .add_template("alice", "alice-normal", emb.clone())
            .expect("add");
        let loaded = store.load("alice").expect("load");
        assert_eq!(loaded.templates.len(), 1);
        assert_eq!(loaded.templates[0].label, "alice-normal");
        assert_eq!(loaded.templates[0].embedding, emb);
    }

    #[test]
    fn remove_template() {
        let (_dir, store) = temp_store();
        store.add_template("bob", "bob-1", vec![1.0]).expect("add");
        store.add_template("bob", "bob-2", vec![2.0]).expect("add");
        store.remove_template("bob", 0).expect("remove");
        let loaded = store.load("bob").expect("load");
        assert_eq!(loaded.templates.len(), 1);
        assert_eq!(loaded.templates[0].id, 0); // re-numbered
        assert_eq!(loaded.templates[0].label, "bob-2");
    }

    #[test]
    fn rejects_path_traversal_usernames() {
        let (_dir, store) = temp_store();
        let err = store
            .add_template("../root", "bad", vec![1.0])
            .expect_err("username rejected");
        assert!(err.to_string().contains("invalid username"));
    }

    #[test]
    fn saved_file_is_private() {
        let (dir, store) = temp_store();
        store
            .add_template("alice", "alice-normal", vec![1.0])
            .expect("add");
        let path = dir.path().join("alice").join("embeddings.json");
        let mode = fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn rejects_world_writable_base_dir() {
        let (dir, store) = temp_store();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).expect("chmod");
        let err = store.load("alice").expect_err("unsafe base dir rejected");
        assert!(err.to_string().contains("unsafe permissions"));
    }
}
