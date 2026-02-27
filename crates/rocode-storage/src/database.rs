use anyhow::Result;
use rocode_types::{MessagePart, PartType};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnection, SqlitePool, SqlitePoolOptions};
use sqlx::{FromRow, Sqlite, Transaction};
use std::future::Future;
use std::path::PathBuf;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("Database connection error: {0}")]
    ConnectionError(String),

    #[error("Migration error: {0}")]
    MigrationError(String),

    #[error("Query error: {0}")]
    QueryError(String),

    #[error("Transaction error: {0}")]
    TransactionError(String),
}

pub struct Database {
    pool: SqlitePool,
}

pub type SqliteTransaction<'a> = Transaction<'a, Sqlite>;

impl Database {
    pub async fn new() -> Result<Self, DatabaseError> {
        let db_path = Self::get_database_path()?;

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;
        }

        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());

        info!("Connecting to database at {}", db_path.display());

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;

        // WAL mode allows concurrent reads during writes; NORMAL sync reduces fsync overhead.
        if let Err(e) = sqlx::query("PRAGMA journal_mode=WAL").execute(&pool).await {
            warn!("failed to set journal_mode=WAL: {}", e);
        }
        if let Err(e) = sqlx::query("PRAGMA synchronous=NORMAL")
            .execute(&pool)
            .await
        {
            warn!("failed to set synchronous=NORMAL: {}", e);
        }

        let db = Self { pool };
        db.run_migrations().await?;

        Ok(db)
    }

    pub async fn in_memory() -> Result<Self, DatabaseError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;

        let db = Self { pool };
        db.run_migrations().await?;

        Ok(db)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn begin(&self) -> Result<SqliteTransaction<'_>, DatabaseError> {
        self.pool
            .begin()
            .await
            .map_err(|e| DatabaseError::TransactionError(e.to_string()))
    }

    pub async fn transaction<F, T, Fut>(&self, f: F) -> Result<T, DatabaseError>
    where
        F: FnOnce(&mut SqliteTransaction<'_>) -> Fut,
        Fut: Future<Output = Result<T, DatabaseError>>,
    {
        let mut tx = self.begin().await?;
        let result = f(&mut tx).await?;
        tx.commit()
            .await
            .map_err(|e| DatabaseError::TransactionError(e.to_string()))?;
        Ok(result)
    }

    pub async fn get_connection(&self) -> Result<SqliteConnection, DatabaseError> {
        self.pool
            .acquire()
            .await
            .map(|conn| conn.detach())
            .map_err(|e| DatabaseError::ConnectionError(e.to_string()))
    }

    async fn run_migrations(&self) -> Result<(), DatabaseError> {
        info!("Running database migrations");

        for migration in crate::schema::ALL_MIGRATIONS {
            match sqlx::query(migration).execute(&self.pool).await {
                Ok(_) => {}
                Err(e) => {
                    let msg = e.to_string();
                    // ALTER TABLE ADD COLUMN fails with "duplicate column" on
                    // databases that already have the column — safe to ignore.
                    if msg.contains("duplicate column") {
                        continue;
                    }
                    return Err(DatabaseError::MigrationError(msg));
                }
            }
        }

        self.run_tool_call_input_data_migration().await?;

        Ok(())
    }

    async fn run_tool_call_input_data_migration(&self) -> Result<(), DatabaseError> {
        #[derive(Debug, FromRow)]
        struct MessageRow {
            id: String,
            data: Option<String>,
        }

        let rows = sqlx::query_as::<_, MessageRow>(
            r#"SELECT id, data
               FROM messages
               WHERE role = 'assistant' AND data IS NOT NULL"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::MigrationError(e.to_string()))?;

        let mut updated_rows = 0usize;
        let mut recovered_inputs = 0usize;
        let mut sentinel_inputs = 0usize;

        for row in rows {
            let Some(data) = row.data else {
                continue;
            };

            let mut parts: Vec<MessagePart> = match serde_json::from_str(&data) {
                Ok(parts) => parts,
                Err(error) => {
                    warn!(
                        message_id = %row.id,
                        %error,
                        "skipping data migration for message with invalid parts JSON"
                    );
                    continue;
                }
            };

            let mut changed = false;
            for part in &mut parts {
                if let PartType::ToolCall { name, input, .. } = &mut part.part_type {
                    let (sanitized, was_recovered, used_sentinel) =
                        sanitize_tool_call_input_for_storage(name, input);
                    if *input != sanitized {
                        *input = sanitized;
                        changed = true;
                    }
                    if was_recovered {
                        recovered_inputs += 1;
                    }
                    if used_sentinel {
                        sentinel_inputs += 1;
                    }
                }
            }

            if !changed {
                continue;
            }

            let next_data = serde_json::to_string(&parts)
                .map_err(|e| DatabaseError::MigrationError(e.to_string()))?;
            sqlx::query("UPDATE messages SET data = ? WHERE id = ?")
                .bind(next_data)
                .bind(&row.id)
                .execute(&self.pool)
                .await
                .map_err(|e| DatabaseError::MigrationError(e.to_string()))?;
            updated_rows += 1;
        }

        if updated_rows > 0 || recovered_inputs > 0 || sentinel_inputs > 0 {
            info!(
                updated_rows,
                recovered_inputs, sentinel_inputs, "tool call input data migration complete"
            );
        }

        Ok(())
    }

    fn get_database_path() -> Result<PathBuf, DatabaseError> {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("opencode");

        Ok(data_dir.join("opencode.db"))
    }
}

fn sanitize_tool_call_input_for_storage(tool_name: &str, input: &Value) -> (Value, bool, bool) {
    if input.is_object() {
        return (input.clone(), false, false);
    }

    if let Some(raw) = input.as_str() {
        if let Some(parsed) = rocode_util::json::try_parse_json_object_robust(raw) {
            return (parsed, true, false);
        }
        if let Some(recovered) =
            rocode_util::json::recover_tool_arguments_from_jsonish(tool_name, raw)
        {
            return (recovered, true, false);
        }

        return (
            serde_json::json!({
                "_rocode_unrecoverable_tool_args": true,
                "tool": tool_name,
                "raw_len": raw.len(),
                "raw_preview": raw.chars().take(240).collect::<String>(),
                "source": "storage-migration",
            }),
            false,
            true,
        );
    }

    let input_type = match input {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::String(_) => "string",
    };
    (
        serde_json::json!({
            "_rocode_unrecoverable_tool_args": true,
            "tool": tool_name,
            "input_type": input_type,
            "source": "storage-migration",
        }),
        false,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::sanitize_tool_call_input_for_storage;

    #[test]
    fn sanitize_tool_call_input_for_storage_recovers_jsonish() {
        let raw = serde_json::Value::String(
            "{\"file_path\":\"t2.html\",\"content\":\"<!DOCTYPE html>".to_string(),
        );
        let (sanitized, recovered, sentinel) = sanitize_tool_call_input_for_storage("write", &raw);
        assert!(sanitized.is_object());
        assert!(recovered);
        assert!(!sentinel);
        assert_eq!(sanitized["file_path"], "t2.html");
    }

    #[test]
    fn sanitize_tool_call_input_for_storage_wraps_unrecoverable() {
        let raw = serde_json::Value::String("not-json".to_string());
        let (sanitized, recovered, sentinel) = sanitize_tool_call_input_for_storage("write", &raw);
        assert!(sanitized.is_object());
        assert!(!recovered);
        assert!(sentinel);
        assert_eq!(sanitized["tool"], "write");
        assert_eq!(sanitized["_rocode_unrecoverable_tool_args"], true);
    }
}
