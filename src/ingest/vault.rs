//! Import one explicitly bounded vault. Source identity includes the vault.

use crate::{
    ingest::{IngestEngine, IngestReport},
    services::structure,
};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::warn;
use walkdir::WalkDir;

const KNOWN_VAULTS: &[&str] = &[
    "Main Vault",
    "Law Modules Vault",
    "Articles Vault",
    "Job Hunt 2026",
];

#[derive(Debug, Clone)]
struct VaultFile {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    checksum: String,
    content: String,
}

#[derive(Debug, Default, Clone)]
pub struct VaultSyncSummary {
    pub scanned: usize,
    pub imported: usize,
    pub skipped_duplicate: usize,
    pub renamed: usize,
    pub errors: Vec<String>,
}

fn scan_vault(root: &Path, scope: &str, max_files: usize) -> Result<Vec<VaultFile>> {
    let mut files = Vec::new();
    let entries = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            if name.starts_with('.') || entry.file_type().is_symlink() {
                return false;
            }
            if entry.file_type().is_dir()
                && (KNOWN_VAULTS.contains(&name.as_ref())
                    || entry.path().join(".obsidian").is_dir())
            {
                return false;
            }
            if scope == "Main Vault"
                && [
                    "International Law",
                    "Administrative Law",
                    "Property Law",
                    "Access to Justice",
                ]
                .contains(&name.as_ref())
            {
                return false;
            }
            true
        });
    for entry in entries {
        let entry = entry.context("walking vault")?;
        let path = entry.path();
        if !entry.file_type().is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = fs::read_to_string(path).context("reading vault source")?;
        let modified_at = DateTime::<Utc>::from(entry.metadata()?.modified()?);
        files.push(VaultFile {
            path: path.into(),
            modified_at,
            checksum: hex::encode(Sha256::digest(content.as_bytes())),
            content,
        });
    }
    files.sort_by(|a, b| b.modified_at.cmp(&a.modified_at).then(a.path.cmp(&b.path)));
    if max_files > 0 {
        files.truncate(max_files);
    }
    Ok(files)
}

pub async fn sync_vault(
    pool: &PgPool,
    vault_path: &Path,
    limit: usize,
    dry_run: bool,
    explicit_scope: Option<&str>,
) -> Result<VaultSyncSummary> {
    let scope = resolve_vault_scope(vault_path, explicit_scope)?;
    let files = scan_vault(vault_path, &scope, limit)?;
    let mut result = VaultSyncSummary {
        scanned: files.len(),
        ..Default::default()
    };
    for file in files {
        let path = normalize_path(&file.path, vault_path, &scope)?;
        if dry_run {
            result.imported += 1;
            continue;
        }
        let title = file
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let section = file
            .path
            .strip_prefix(vault_path)?
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string());
        let meta = serde_json::json!({"source":"obsidian-vault","vault_scope":scope,"source_path":file.path,"source_last_modified_at":file.modified_at});
        // Never infer a rename from equal text: distinct vaults or files can
        // intentionally contain identical notes. Legacy ambiguous rows remain.
        let changed = sqlx::query(
            "INSERT INTO documents (path,title,content,checksum,vault_section,frontmatter,file_modified_at,file_size_bytes) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             ON CONFLICT(path) DO UPDATE SET title=EXCLUDED.title,content=EXCLUDED.content,checksum=EXCLUDED.checksum, \
                vault_section=EXCLUDED.vault_section,frontmatter=coalesce(documents.frontmatter,'{}'::jsonb)||EXCLUDED.frontmatter, \
                file_modified_at=EXCLUDED.file_modified_at,file_size_bytes=EXCLUDED.file_size_bytes,updated_at=now() \
             WHERE (documents.content,documents.title,documents.checksum,documents.file_modified_at) \
                IS DISTINCT FROM (EXCLUDED.content,EXCLUDED.title,EXCLUDED.checksum,EXCLUDED.file_modified_at) \
                OR NOT coalesce(documents.frontmatter,'{}'::jsonb) @> EXCLUDED.frontmatter",
        ).bind(&path).bind(&title).bind(&file.content).bind(&file.checksum).bind(section).bind(meta)
            .bind(file.modified_at).bind(i32::try_from(file.content.len())?).execute(pool).await?;
        if changed.rows_affected() == 0 {
            result.skipped_duplicate += 1;
        } else {
            result.imported += 1;
        }
        let row = sqlx::query("SELECT id,storage_tier FROM documents WHERE path=$1")
            .bind(&path)
            .fetch_one(pool)
            .await?;
        if let Err(error) = structure::rebuild_document_nodes(
            pool,
            row.get("id"),
            &path,
            &title,
            &file.content,
            Some(&file.checksum),
            Some(file.modified_at),
            &scope,
            &row.get::<String, _>("storage_tier"),
        )
        .await
        {
            warn!("Source saved; hierarchy needs retry: {error}");
            result
                .errors
                .push(format!("hierarchy rebuild failed for {path}"));
        }
    }
    Ok(result)
}

pub async fn ingest_vault(
    engine: &IngestEngine,
    vault_path: &Path,
    limit: usize,
    dry_run: bool,
    explicit_scope: Option<&str>,
    report: &mut IngestReport,
) -> Result<VaultSyncSummary> {
    let summary = sync_vault(engine.pool(), vault_path, limit, dry_run, explicit_scope).await?;
    report
        .sources_processed
        .insert("obsidian-vault".into(), summary.scanned as u64);
    report.documents_created += summary.imported as u64;
    report.errors += summary.errors.len() as u64;
    Ok(summary)
}

fn resolve_vault_scope(root: &Path, explicit: Option<&str>) -> Result<String> {
    let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let known = KNOWN_VAULTS.contains(&name);
    ensure!(
        known || !KNOWN_VAULTS.iter().any(|n| root.join(n).is_dir()),
        "select a single vault directory; a scope label cannot authorize importing multiple vaults"
    );
    if let Some(scope) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        ensure!(
            !known || scope == name,
            "explicit scope disagrees with the selected vault root"
        );
        return Ok(scope.into());
    }
    ensure!(
        known || root.join(".obsidian").is_dir(),
        "select a vault root or provide its explicit scope"
    );
    ensure!(!name.is_empty(), "vault root has no name");
    Ok(name.into())
}

fn normalize_path(path: &Path, root: &Path, scope: &str) -> Result<String> {
    let rel = path.strip_prefix(root)?;
    let encode = |value: &str| -> String {
        value
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    };
    let parts = rel
        .components()
        .map(|c| encode(&c.as_os_str().to_string_lossy()))
        .collect::<Vec<_>>();
    Ok(format!("vault://{}/{}", encode(scope), parts.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_is_part_of_source_identity() {
        let a = normalize_path(
            Path::new("/vault/note.md"),
            Path::new("/vault"),
            "Main Vault",
        )
        .unwrap();
        let b = normalize_path(
            Path::new("/vault/note.md"),
            Path::new("/vault"),
            "Law Modules Vault",
        )
        .unwrap();
        assert_ne!(a, b);
    }
    #[test]
    fn explicit_label_cannot_override_multi_vault_parent_or_known_vault() {
        let root = std::env::temp_dir().join(format!("vault-scope-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("Main Vault")).unwrap();
        fs::create_dir_all(root.join("Law Modules Vault")).unwrap();
        assert!(resolve_vault_scope(&root, Some("Main Vault")).is_err());
        assert!(resolve_vault_scope(&root.join("Main Vault"), Some("Law Modules Vault")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn nested_vault_and_generated_files_are_pruned() {
        let root = std::env::temp_dir()
            .join(format!("vault-scan-{}", uuid::Uuid::new_v4()))
            .join("Main Vault");
        fs::create_dir_all(root.join("Law Modules Vault")).unwrap();
        fs::create_dir_all(root.join(".playwright-mcp")).unwrap();
        fs::write(root.join("keep.md"), "project").unwrap();
        fs::write(root.join("Law Modules Vault/private.md"), "law").unwrap();
        fs::write(root.join(".playwright-mcp/browser.md"), "generated").unwrap();
        let found = scan_vault(&root, "Main Vault", 0).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("keep.md"));
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }
}
