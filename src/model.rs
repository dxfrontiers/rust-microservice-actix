use actix_web::{HttpRequest, HttpResponse, Responder, ResponseError};
use bb8::RunError;
use futures::future::{ready, Ready};
use prometheus::Histogram;
use serde::export::Formatter;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::error::Error;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio_postgres::Row;
use validator::Validate;
use validator_derive::Validate;

/*
This is the newtype pattern, that seems to be our best option here
https://doc.rust-lang.org/1.0.0/style/features/types/newtype.html
*/
#[derive(Debug, Validate, Deserialize, Serialize)]
pub struct NewPrintJob {
    pub user_id: i32,
    #[validate(range(min = 0, max = 1000))] // some pretty dumb restriction but ok
    pub file_id: i32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PrintJob {
    pub job_id: i32,
    pub user_id: i32,
    pub file_id: i32,
    pub state: i32,
}

#[derive(Debug)]
pub enum PrintJobError {
    UnparseablePrintJob(String),
    DatabaseError(String),
    BackendError(String),
}

// The Responder trait allows us to answer HTTP requests directly with a PrintJob
impl Responder for PrintJob {
    type Error = actix_web::Error;
    type Future = Ready<Result<HttpResponse, Self::Error>>;

    fn respond_to(self, _: &HttpRequest) -> Self::Future {
        let body = serde_json::to_string(&self);
        match body {
            Ok(body) => ready(Ok(HttpResponse::Ok()
                .content_type("application/json")
                .body(body))),
            Err(_) => ready(Ok(HttpResponse::InternalServerError().finish())),
        }
    }
}

// With ResponseError and Error implemented, we can answer HTTP requests directly with a PrintJobError
impl ResponseError for PrintJobError {}

impl Error for PrintJobError {}

// For casting specific errors to our custom ones
impl From<tokio_postgres::error::Error> for PrintJobError {
    fn from(e: tokio_postgres::error::Error) -> Self {
        PrintJobError::DatabaseError(e.to_string())
    }
}

// For casting specific errors to our custom ones
impl From<bb8::RunError<tokio_postgres::error::Error>> for PrintJobError {
    fn from(e: RunError<tokio_postgres::error::Error>) -> Self {
        PrintJobError::DatabaseError(e.to_string())
    }
}

impl Display for PrintJobError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "PrintJobError: {:?}", self)
    }
}

// This allows us to try to create PrintJob from the postgres row
impl TryFrom<Row> for PrintJob {
    type Error = PrintJobError;

    fn try_from(row: Row) -> Result<Self, Self::Error> {
        let job_id = row.try_get::<usize, i32>(0)?;
        let user_id = row.try_get::<usize, i32>(1)?;
        let file_id = row.try_get::<usize, i32>(2)?;
        let state = row.try_get::<usize, i32>(3)?;

        // do validations
        // dummy validation:
        if state > 60 {
            return Err(PrintJobError::UnparseablePrintJob("Bröken".to_string()));
        }

        Ok(Self {
            job_id,
            user_id,
            file_id,
            state,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ManagementLookupResponse {
    pub url: String,
    pub printer_id: i32,
    pub auth_result: i32,
}

/*
*/
#[derive(Clone)]
pub struct RequestCounter {
    sem: Arc<Semaphore>,
}
impl RequestCounter {
    pub fn new(permit_amount: usize, metric: Histogram) -> Self {
        let sem = Arc::new(Semaphore::new(permit_amount));

        let metrics_sem = sem.clone();

        // This is far from perfect and is used to update the metrics. That way, metrics reporting does not affect the request performance
        actix_rt::spawn(async move {
            let mut interval = actix_rt::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                metric.observe(metrics_sem.available_permits() as f64);
            }
        });

        RequestCounter { sem }
    }

    // Return a permit if the protected area still has leases
    pub fn enter(&self) -> Option<SemaphorePermit> {
        let res = self.sem.try_acquire().ok();
        res
    }
}
