//! Storage. Each Document's PDF is a file, `<doc_id>.pdf`; Document metadata and
//! every Conversation live in a SQLite database, `library.db`, in the same folder.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::claude::{Citation, Role};
use crate::prompts::ReaderContext;

/// A Document as the library lists it.
#[derive(Clone, Debug, Serialize)]
pub struct DocumentMeta {
    pub doc_id: String,
    pub filename: String,
    pub page_count: usize,
    pub size_bytes: usize,
    pub uploaded_at: String,
    /// Time of the latest Turn, or the upload time if there are none.
    pub last_active_at: String,
    /// Questions asked so far in this Document's Conversation.
    pub question_count: usize,
    /// The Files API id of the uploaded PDF, once it has been sent to Claude.
    #[serde(skip)]
    pub file_id: Option<String>,
}

/// One saved Turn of a Conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredTurn {
    pub role: Role,
    pub content: String,
    /// User Turns: where the Reader was when they asked.
    pub context: Option<ReaderContext>,
    /// Answers: the model that wrote it.
    pub model: Option<String>,
    /// Answers: true when a Fallback model answered.
    pub used_fallback: bool,
    /// Answers: the passages the Answer cites, numbered as its markers are.
    pub citations: Vec<Citation>,
    pub created_at: String,
}

impl StoredTurn {
    pub fn question(content: String, context: ReaderContext) -> Self {
        Self {
            role: Role::User,
            content,
            context: Some(context),
            model: None,
            used_fallback: false,
            citations: Vec::new(),
            created_at: now(),
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    NotFound,
    Io(io::Error),
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            StoreError::NotFound
        } else {
            StoreError::Io(e)
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound,
            e => StoreError::Io(io::Error::other(e)),
        }
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Io(io::Error::other(e))
    }
}

/// doc_ids are 32 lowercase hex chars; anything else is rejected (and can't traverse paths).
pub fn valid_doc_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS documents (
    doc_id      TEXT PRIMARY KEY,
    filename    TEXT NOT NULL,
    page_count  INTEGER NOT NULL,
    size_bytes  INTEGER NOT NULL,
    uploaded_at TEXT NOT NULL,
    file_id     TEXT
);
CREATE TABLE IF NOT EXISTS turns (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id        TEXT NOT NULL REFERENCES documents(doc_id) ON DELETE CASCADE,
    role          TEXT NOT NULL,
    content       TEXT NOT NULL,
    context       TEXT,
    model         TEXT,
    used_fallback INTEGER NOT NULL DEFAULT 0,
    citations     TEXT NOT NULL DEFAULT '[]',
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS turns_by_doc ON turns(doc_id, id);
";

const SELECT_META: &str = "
SELECT d.doc_id, d.filename, d.page_count, d.size_bytes, d.uploaded_at, d.file_id,
       COALESCE(MAX(t.created_at), d.uploaded_at),
       COUNT(CASE WHEN t.role = 'user' THEN 1 END)
FROM documents d LEFT JOIN turns t ON t.doc_id = d.doc_id
";

fn meta_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentMeta> {
    Ok(DocumentMeta {
        doc_id: row.get(0)?,
        filename: row.get(1)?,
        page_count: row.get::<_, i64>(2)?.max(0) as usize,
        size_bytes: row.get::<_, i64>(3)?.max(0) as usize,
        uploaded_at: row.get(4)?,
        file_id: row.get(5)?,
        last_active_at: row.get(6)?,
        question_count: row.get::<_, i64>(7)?.max(0) as usize,
    })
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

#[derive(Clone, Debug)]
pub struct DocumentStore {
    root: PathBuf,
    db: Arc<Mutex<Connection>>,
}

impl DocumentStore {
    /// Open (or create) the store in `root`, importing Documents saved by older
    /// versions as `<doc_id>.json` files.
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        let conn = Connection::open(root.join("library.db")).map_err(io::Error::other)?;
        conn.execute_batch(SCHEMA).map_err(io::Error::other)?;
        let store = Self {
            root,
            db: Arc::new(Mutex::new(conn)),
        };
        store.import_legacy_metadata()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn pdf_path(&self, doc_id: &str) -> Result<PathBuf, StoreError> {
        if !valid_doc_id(doc_id) {
            return Err(StoreError::NotFound);
        }
        Ok(self.root.join(format!("{doc_id}.pdf")))
    }

    /// Run `f` on the database off the async runtime.
    async fn with_db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = db.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut conn)
        })
        .await
        .map_err(|e| StoreError::Io(io::Error::other(e)))?
    }

    fn import_legacy_metadata(&self) -> io::Result<()> {
        #[derive(Deserialize)]
        struct Legacy {
            doc_id: String,
            filename: String,
            page_count: usize,
            size_bytes: usize,
            uploaded_at: String,
        }
        let conn = self.db.lock().unwrap_or_else(|p| p.into_inner());
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(m) = std::fs::read(&path)
                .map_err(io::Error::other)
                .and_then(|raw| serde_json::from_slice::<Legacy>(&raw).map_err(io::Error::other))
            else {
                continue;
            };
            if !valid_doc_id(&m.doc_id) || !self.root.join(format!("{}.pdf", m.doc_id)).exists() {
                continue;
            }
            conn.execute(
                "INSERT OR IGNORE INTO documents (doc_id, filename, page_count, size_bytes, uploaded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![m.doc_id, m.filename, m.page_count as i64, m.size_bytes as i64, m.uploaded_at],
            )
            .map_err(io::Error::other)?;
            std::fs::remove_file(&path)?;
            tracing::info!(doc_id = %m.doc_id, "imported document metadata into library.db");
        }
        Ok(())
    }

    pub async fn save(
        &self,
        data: &[u8],
        filename: &str,
        page_count: usize,
    ) -> Result<DocumentMeta, StoreError> {
        let doc_id = uuid::Uuid::new_v4().simple().to_string();
        tokio::fs::write(self.pdf_path(&doc_id)?, data).await?;
        let uploaded_at = now();
        let meta = DocumentMeta {
            doc_id: doc_id.clone(),
            filename: filename.to_string(),
            page_count,
            size_bytes: data.len(),
            uploaded_at: uploaded_at.clone(),
            last_active_at: uploaded_at,
            question_count: 0,
            file_id: None,
        };
        let m = meta.clone();
        self.with_db(move |c| {
            c.execute(
                "INSERT INTO documents (doc_id, filename, page_count, size_bytes, uploaded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    m.doc_id,
                    m.filename,
                    m.page_count as i64,
                    m.size_bytes as i64,
                    m.uploaded_at
                ],
            )?;
            Ok(())
        })
        .await?;
        Ok(meta)
    }

    pub async fn get_meta(&self, doc_id: &str) -> Result<DocumentMeta, StoreError> {
        if !valid_doc_id(doc_id) {
            return Err(StoreError::NotFound);
        }
        let id = doc_id.to_string();
        self.with_db(move |c| {
            Ok(c.query_row(
                &format!("{SELECT_META} WHERE d.doc_id = ?1 GROUP BY d.doc_id"),
                [id],
                meta_from_row,
            )?)
        })
        .await
    }

    /// Every Document, most recently active first.
    pub async fn list(&self) -> Result<Vec<DocumentMeta>, StoreError> {
        self.with_db(|c| {
            let mut stmt = c.prepare(&format!(
                "{SELECT_META} GROUP BY d.doc_id ORDER BY 7 DESC, 1"
            ))?;
            let rows = stmt.query_map([], meta_from_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }

    pub async fn get_pdf(&self, doc_id: &str) -> Result<Vec<u8>, StoreError> {
        Ok(tokio::fs::read(self.pdf_path(doc_id)?).await?)
    }

    /// Delete a Document, its PDF and its Conversation. Returns its Files API id, if any,
    /// so the caller can delete the remote copy too.
    pub async fn delete(&self, doc_id: &str) -> Result<Option<String>, StoreError> {
        let path = self.pdf_path(doc_id)?;
        let id = doc_id.to_string();
        let file_id = self
            .with_db(move |c| {
                let file_id: Option<String> = c
                    .query_row(
                        "SELECT file_id FROM documents WHERE doc_id = ?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or(StoreError::NotFound)?;
                c.execute("DELETE FROM documents WHERE doc_id = ?1", [&id])?;
                Ok(file_id)
            })
            .await?;
        match tokio::fs::remove_file(path).await {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        Ok(file_id)
    }

    pub async fn set_file_id(&self, doc_id: &str, file_id: Option<&str>) -> Result<(), StoreError> {
        let id = doc_id.to_string();
        let file_id = file_id.map(str::to_string);
        self.with_db(move |c| {
            c.execute(
                "UPDATE documents SET file_id = ?2 WHERE doc_id = ?1",
                params![id, file_id],
            )?;
            Ok(())
        })
        .await
    }

    /// The Document's Conversation, oldest Turn first.
    pub async fn turns(&self, doc_id: &str) -> Result<Vec<StoredTurn>, StoreError> {
        let id = doc_id.to_string();
        self.with_db(move |c| {
            let mut stmt = c.prepare(
                "SELECT role, content, context, model, used_fallback, citations, created_at
                 FROM turns WHERE doc_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, bool>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (role, content, context, model, used_fallback, citations, created_at) = row?;
                out.push(StoredTurn {
                    role: if role == "user" {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    content,
                    context: context.as_deref().map(serde_json::from_str).transpose()?,
                    model,
                    used_fallback,
                    citations: serde_json::from_str(&citations)?,
                    created_at,
                });
            }
            Ok(out)
        })
        .await
    }

    /// Save a Question and its Answer together, so a Conversation never ends mid-exchange.
    pub async fn append_exchange(
        &self,
        doc_id: &str,
        question: StoredTurn,
        answer: StoredTurn,
    ) -> Result<(), StoreError> {
        let id = doc_id.to_string();
        self.with_db(move |c| {
            let tx = c.transaction()?;
            for t in [question, answer] {
                tx.execute(
                    "INSERT INTO turns (doc_id, role, content, context, model, used_fallback, citations, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        id,
                        role_str(t.role),
                        t.content,
                        t.context.as_ref().map(serde_json::to_string).transpose()?,
                        t.model,
                        t.used_fallback,
                        serde_json::to_string(&t.citations)?,
                        t.created_at,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn clear_conversation(&self, doc_id: &str) -> Result<(), StoreError> {
        let id = doc_id.to_string();
        self.with_db(move |c| {
            c.execute("DELETE FROM turns WHERE doc_id = ?1", [id])?;
            Ok(())
        })
        .await
    }
}
