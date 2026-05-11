use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

use crate::error::{FaceRsError, Result};
use crate::matching::Embedding;

#[derive(Clone, Serialize, Deserialize)]
pub struct EnrolledTemplate {
    pub id: u32,
    pub label: String,
    pub created_at: String,
    #[serde(default = "default_template_scope")]
    pub scope: TemplateScope,
    pub embedding: Embedding,
}

impl fmt::Debug for EnrolledTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrolledTemplate")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("created_at", &self.created_at)
            .field("scope", &self.scope)
            .field("embedding", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateScope {
    Sudo,
    Session,
    Both,
}

impl TemplateScope {
    pub fn allows(self, auth_scope: AuthScope) -> bool {
        matches!(
            (self, auth_scope),
            (TemplateScope::Both, _)
                | (TemplateScope::Sudo, AuthScope::Sudo)
                | (TemplateScope::Session, AuthScope::Session)
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            TemplateScope::Sudo => "sudo",
            TemplateScope::Session => "session",
            TemplateScope::Both => "both",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScope {
    Sudo,
    Session,
}

fn default_template_scope() -> TemplateScope {
    TemplateScope::Both
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct UserTemplates {
    pub templates: Vec<EnrolledTemplate>,
    /// Monotonic counter — the next id we'll hand out. Persisted so we never
    /// reuse the id of a removed template, which kept callers (CLI scripts,
    /// the TUI delete flow) safe from referencing a stale id.
    /// `#[serde(default)]` keeps older `embeddings.json` files (without this
    /// field) loadable: we recompute it from `templates` on first load.
    #[serde(default)]
    pub next_id: u32,
}

impl fmt::Debug for UserTemplates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserTemplates")
            .field("templates", &self.templates)
            .field("next_id", &self.next_id)
            .finish()
    }
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
            fs::set_permissions(&self.base_dir, fs::Permissions::from_mode(0o700)).map_err(
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
        fs::File::open(dir)
            .and_then(|dir_file| dir_file.sync_all())
            .map_err(|e| FaceRsError::Storage(format!("cannot sync {}: {e}", dir.display())))?;
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
        scope: TemplateScope,
        embedding: Embedding,
    ) -> Result<EnrolledTemplate> {
        let mut store = self.load(username)?;
        // Heal legacy stores written before next_id was persisted.
        let observed_max = store.templates.iter().map(|t| t.id).max();
        if let Some(m) = observed_max {
            store.next_id = store.next_id.max(m.saturating_add(1));
        }
        let id = store.next_id;
        store.next_id = store.next_id.saturating_add(1);
        let created_at = chrono_now();
        let template = EnrolledTemplate {
            id,
            label: label.to_owned(),
            created_at,
            scope,
            embedding,
        };
        store.templates.push(template.clone());
        self.save(username, &store)?;
        zeroize_templates(&mut store.templates);
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
        // IDs are not renumbered: callers (CLI, TUI) reference templates by ID
        // and renumbering after a delete shifts all subsequent IDs, surprising
        // the user and breaking any in-flight script.
        let result = self.save(username, &store);
        zeroize_templates(&mut store.templates);
        result
    }

    pub fn embeddings_for(&self, username: &str) -> Result<Vec<Embedding>> {
        let store = self.load(username)?;
        if store.templates.is_empty() {
            return Err(FaceRsError::NotEnrolled);
        }
        Ok(store.templates.into_iter().map(|t| t.embedding).collect())
    }

    pub fn embeddings_for_scope(
        &self,
        username: &str,
        auth_scope: AuthScope,
    ) -> Result<Vec<Embedding>> {
        let store = self.load(username)?;
        let embeddings = store
            .templates
            .into_iter()
            .filter(|t| t.scope.allows(auth_scope))
            .map(|t| t.embedding)
            .collect::<Vec<_>>();
        if embeddings.is_empty() {
            return Err(FaceRsError::NotEnrolled);
        }
        Ok(embeddings)
    }
}

fn zeroize_templates(templates: &mut [EnrolledTemplate]) {
    for template in templates {
        template.embedding.zeroize();
    }
}

/// Returns a UTC RFC-3339 timestamp like `2026-05-10T12:34:56Z`. Implemented
/// by hand to avoid pulling in chrono/time for a single string.
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_utc(secs as i64)
}

fn format_unix_utc(secs: i64) -> String {
    // Days since 1970-01-01.
    let mut days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let h = (secs_of_day / 3600) as u32;
    let m = ((secs_of_day % 3600) / 60) as u32;
    let s = (secs_of_day % 60) as u32;

    // Walk forward from 1970 in years, accounting for leap years.
    let mut year: i32 = 1970;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    static MONTH_DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0usize;
    while month < 12 {
        let mut dm = MONTH_DAYS[month] as i64;
        if month == 1 && is_leap(year) {
            dm += 1;
        }
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    let day = days as u32 + 1;
    let month_num = month as u32 + 1;
    format!("{year:04}-{month_num:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
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
            .add_template("alice", "alice-normal", TemplateScope::Both, emb.clone())
            .expect("add");
        let loaded = store.load("alice").expect("load");
        assert_eq!(loaded.templates.len(), 1);
        assert_eq!(loaded.templates[0].label, "alice-normal");
        assert_eq!(loaded.templates[0].embedding, emb);
    }

    #[test]
    fn remove_template() {
        let (_dir, store) = temp_store();
        store
            .add_template("bob", "bob-1", TemplateScope::Both, vec![1.0])
            .expect("add");
        store
            .add_template("bob", "bob-2", TemplateScope::Both, vec![2.0])
            .expect("add");
        store.remove_template("bob", 0).expect("remove");
        let loaded = store.load("bob").expect("load");
        assert_eq!(loaded.templates.len(), 1);
        // IDs are stable across deletes; the surviving template keeps its id.
        assert_eq!(loaded.templates[0].id, 1);
        assert_eq!(loaded.templates[0].label, "bob-2");
    }

    #[test]
    fn add_after_remove_does_not_reuse_id() {
        let (_dir, store) = temp_store();
        store
            .add_template("eve", "eve-1", TemplateScope::Both, vec![1.0])
            .expect("add");
        store
            .add_template("eve", "eve-2", TemplateScope::Both, vec![2.0])
            .expect("add");
        store.remove_template("eve", 1).expect("remove");
        let new = store
            .add_template("eve", "eve-3", TemplateScope::Both, vec![3.0])
            .expect("add");
        assert_eq!(new.id, 2);
    }

    #[test]
    fn filters_embeddings_by_auth_scope() {
        let (_dir, store) = temp_store();
        store
            .add_template("alice", "sudo", TemplateScope::Sudo, vec![1.0])
            .expect("add");
        store
            .add_template("alice", "session", TemplateScope::Session, vec![2.0])
            .expect("add");
        store
            .add_template("alice", "both", TemplateScope::Both, vec![3.0])
            .expect("add");

        let sudo = store
            .embeddings_for_scope("alice", AuthScope::Sudo)
            .expect("sudo embeddings");
        assert_eq!(sudo, vec![vec![1.0], vec![3.0]]);

        let session = store
            .embeddings_for_scope("alice", AuthScope::Session)
            .expect("session embeddings");
        assert_eq!(session, vec![vec![2.0], vec![3.0]]);
    }

    #[test]
    fn rejects_path_traversal_usernames() {
        let (_dir, store) = temp_store();
        let err = store
            .add_template("../root", "bad", TemplateScope::Both, vec![1.0])
            .expect_err("username rejected");
        assert!(err.to_string().contains("invalid username"));
    }

    #[test]
    fn saved_file_is_private() {
        let (dir, store) = temp_store();
        store
            .add_template("alice", "alice-normal", TemplateScope::Both, vec![1.0])
            .expect("add");
        let path = dir.path().join("alice").join("embeddings.json");
        let mode = fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn format_unix_utc_known_values() {
        // 2024-01-01T00:00:00Z = 1704067200
        assert_eq!(format_unix_utc(1_704_067_200), "2024-01-01T00:00:00Z");
        // 2026-05-10T00:00:00Z = 1778371200
        assert_eq!(format_unix_utc(1_778_371_200), "2026-05-10T00:00:00Z");
        // Leap day: 2024-02-29T12:34:56Z = 1709210096
        assert_eq!(format_unix_utc(1_709_210_096), "2024-02-29T12:34:56Z");
    }

    #[test]
    fn debug_redacts_embeddings() {
        let template = EnrolledTemplate {
            id: 1,
            label: "front".to_owned(),
            created_at: "2026-05-11T00:00:00Z".to_owned(),
            scope: TemplateScope::Both,
            embedding: vec![0.123456, 0.654321],
        };
        let debug = format!("{template:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("0.123456"));
        assert!(!debug.contains("0.654321"));
    }

    #[test]
    fn rejects_world_writable_base_dir() {
        let (dir, store) = temp_store();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).expect("chmod");
        let err = store.load("alice").expect_err("unsafe base dir rejected");
        assert!(err.to_string().contains("unsafe permissions"));
    }
}
