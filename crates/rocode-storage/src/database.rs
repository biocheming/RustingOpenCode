use anyhow::Result;
use sqlx::sqlite::{SqliteConnection, SqlitePool, SqlitePoolOptions};
use sqlx::{Sqlite, Transaction};
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
        if let Err(e) = sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await
        {
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

        Ok(())
    }

    fn get_database_path() -> Result<PathBuf, DatabaseError> {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("opencode");

        Ok(data_dir.join("opencode.db"))
    }
}
