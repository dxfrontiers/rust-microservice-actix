use crate::model::{NewPrintJob, PrintJob, PrintJobError};
use bb8::Pool;
use bb8_postgres::PostgresConnectionManager;
use std::convert::TryFrom;
use std::str::FromStr;
use tokio_postgres::types::Type;
use tokio_postgres::NoTls;

use log::warn;

#[derive(Clone)]
pub struct Storage {
    pool: Pool<PostgresConnectionManager<NoTls>>,
}

const DEFAULT_START_STATE: i32 = 0;

impl Storage {
    pub async fn select_print_job_by_id(&self, id: i32) -> Result<Option<PrintJob>, PrintJobError> {
        let connection = self.pool.get().await?;
        let stmt = connection
            .prepare_typed("SELECT * FROM jobs WHERE jobid = $1", &[Type::INT4])
            .await?;
        let row = connection.query_opt(&stmt, &[&id]).await?;

        row.map_or(Ok(None), |row| PrintJob::try_from(row).map(|e| Some(e)))
    }

    pub async fn create_new_print_job(&self, job: NewPrintJob) -> Result<PrintJob, PrintJobError> {
        let connection = self.pool.get().await?;
        let stmt = connection
            .prepare_typed(
                "INSERT INTO jobs (uid, fid, state) VALUES($1,$2,$3) RETURNING *",
                &[Type::INT4, Type::INT4, Type::INT4],
            )
            .await?;
        let row = connection
            .query_opt(&stmt, &[&job.user_id, &job.file_id, &DEFAULT_START_STATE])
            .await?;

        match row {
            Some(row) => PrintJob::try_from(row),
            None => {
                warn!("insert failed for uid {}", job.user_id);
                Err(PrintJobError::DatabaseError("Insert failed".to_string()))
            }
        }
    }

    pub async fn new(pool: Pool<PostgresConnectionManager<NoTls>>) -> Self {
        Storage { pool }
    }
}

// we wrap the BB8 pool in out struct with concrete types
pub struct DbPool {
    pub pool: Pool<PostgresConnectionManager<NoTls>>,
}

pub enum DBError {
    ConnectionError(String),
}

impl ToString for DBError {
    fn to_string(&self) -> String {
        match self {
            DBError::ConnectionError(s) => s.clone(),
        }
    }
}

impl DbPool {
    pub async fn new_from_pg_str(pg_str: String) -> Result<DbPool, DBError> {
        let config = tokio_postgres::config::Config::from_str(&pg_str)
            .expect("Could not connect to database");

        let pg_mgr = PostgresConnectionManager::new(config, tokio_postgres::NoTls);

        let pool = Pool::builder()
            .build(pg_mgr)
            .await
            .map_err(|e| DBError::ConnectionError(e.to_string()))?;

        Ok(DbPool { pool })
    }
}
