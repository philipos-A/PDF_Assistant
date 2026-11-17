//! On-disk storage: each document is `<doc_id>.pdf` plus `<doc_id>.json` metadata.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub doc_id: String,
    pub filename: String,
    pub page_count: usize,
    pub size_bytes: usize,
    pub uploaded_at: String,
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

#[derive(Clone, Debug)]
pub struct DocumentStore {
    root: PathBuf,
}

/// doc_ids are 32 lowercase hex chars; anything else is rejected (and can't traverse paths).
pub fn valid_doc_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl DocumentStore {
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, doc_id: &str, ext: &str) -> Result<PathBuf, StoreError> {
        if !valid_doc_id(doc_id) {
            return Err(StoreError::NotFound);
        }
        Ok(self.root.join(format!("{doc_id}.{ext}")))
    }

    pub async fn save(
        &self,
        data: &[u8],
        filename: &str,
        page_count: usize,
    ) -> Result<DocumentMeta, StoreError> {
        let doc_id = uuid::Uuid::new_v4().simple().to_string();
        let meta = DocumentMeta {
            doc_id: doc_id.clone(),
            filename: filename.to_string(),
            page_count,
            size_bytes: data.len(),
            uploaded_at: chrono::Utc::now().to_rfc3339(),
        };
        tokio::fs::write(self.path(&doc_id, "pdf")?, data).await?;
        let json = serde_json::to_vec(&meta).map_err(|e| StoreError::Io(io::Error::other(e)))?;
        tokio::fs::write(self.path(&doc_id, "json")?, json).await?;
        Ok(meta)
    }

    pub async fn get_meta(&self, doc_id: &str) -> Result<DocumentMeta, StoreError> {
        let raw = tokio::fs::read(self.path(doc_id, "json")?).await?;
        serde_json::from_slice(&raw).map_err(|e| StoreError::Io(io::Error::other(e)))
    }

    pub async fn get_pdf(&self, doc_id: &str) -> Result<Vec<u8>, StoreError> {
        Ok(tokio::fs::read(self.path(doc_id, "pdf")?).await?)
    }
}
