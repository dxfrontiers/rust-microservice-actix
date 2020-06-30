use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer, Responder};

use crate::model::RequestCounter;
use crate::model::{ManagementLookupResponse, NewPrintJob, PrintJobError};
use crate::storage::{DbPool, Storage};
use actix_web_prom::PrometheusMetrics;
use hyper::http::StatusCode;
use log::info;
use prometheus::{Histogram, HistogramOpts};

mod model;
mod storage;

async fn post_print_job(
    job: web::Json<NewPrintJob>,
    storage: web::Data<Storage>,
    request_counter: web::Data<RequestCounter>,
) -> Result<HttpResponse, PrintJobError> {
    // We should probably validate the job here
    //job.validate().map_err::<_,HttpResponse>(|e| Err(HttpResponse::BadRequest().body(format!("Bröken: {}",e))))?;

    let job = job.into_inner();

    info!("Got print job posted by user_id: {}", job.user_id);

    /*
       This function might pile up work in tokio tasks, so we limit the total amount of concurrent executions
    */
    let permit = request_counter.enter();
    if permit.is_none() {
        return Ok(HttpResponse::TooManyRequests().body("slow down buddy"));
    }

    let response = start_lookup_request(&job).await;

    let response = match response {
        Ok(response) if response.status() == StatusCode::OK => {
            let _backend_lookup_result = response
                .json::<ManagementLookupResponse>()
                .await
                .map_err(|e| PrintJobError::BackendError(format!("Backend broken {}", e)))?;
            // here we would do something with backend_lookup_result

            storage.create_new_print_job(job).await.map(|job| {
                HttpResponse::Ok()
                    .content_type("application/json")
                    .body(serde_json::to_string(&job).unwrap_or("{}".to_string()))
            })
        }
        Ok(response) if response.status() == StatusCode::NOT_FOUND => {
            Ok(HttpResponse::NotFound().body(format!("Nix da")))
        }
        Ok(_) => Ok(HttpResponse::InternalServerError().body(format!("Dafuq?:"))),
        Err(err) => Err(err),
    };

    response
}

/**
 * Start a lookup at the management backend
 */
async fn start_lookup_request(job: &NewPrintJob) -> Result<reqwest::Response, PrintJobError> {
    reqwest::Client::new()
        .get("http://127.0.0.1:8090/lookup")
        .json(&job)
        .send()
        .await
        .map_err(|e| PrintJobError::BackendError(format!("Backend unavailable: {}", e)))
}

async fn get_print_job_by_id(id: web::Path<i32>, storage: web::Data<Storage>) -> impl Responder {
    storage.select_print_job_by_id(id.into_inner()).await
}

async fn health_probe(_: HttpRequest) -> HttpResponse {
    HttpResponse::Ok().finish()
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    // we obviously want to fetch this from the config
    let pool = DbPool::new_from_pg_str("postgresql://management@localhost:5432".into())
        .await
        .unwrap_or_else(|_| panic!("Could not get DB connection"));

    let storage = Storage::new(pool.pool).await;

    // metrics stuff that is hardcoded
    let prometheus = PrometheusMetrics::new("printer", Some("/metrics"), None);
    let hist_opt = HistogramOpts::new(
        "pending_backend_requests",
        "The ratio of used pending backend requests",
    );
    let hist = Histogram::with_opts(hist_opt).unwrap();

    prometheus
        .registry
        .register(Box::new(hist.clone()))
        .unwrap();

    // this one ensures that only 10 backend requests run at the same time
    let pending_request_counter = RequestCounter::new(10, hist);

    HttpServer::new(move || {
        App::new()
            .data(storage.clone())
            .data(pending_request_counter.clone())
            .wrap(middleware::Logger::default())
            .wrap(prometheus.clone())
            .route("/print/jobs", web::post().to(post_print_job))
            .route("/print/jobs/{id}", web::get().to(get_print_job_by_id))
            .route("/health", web::get().to(health_probe))
    })
    .bind("127.0.0.1:8088")?
    .run()
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http, test, web, App};

    #[actix_rt::test]
    async fn test_status_ok() {
        let req = test::TestRequest::default().to_http_request();
        let resp = super::health_probe(req).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
    }

    /**
     * For this test to work, the database has to be populated
     */
    #[actix_rt::test]
    async fn test_print_get_response() {
        let pool = DbPool::new_from_pg_str("postgresql://management@localhost:5432".into())
            .await
            .unwrap_or_else(|_| panic!("Could not get DB connection"));
        let storage = Storage::new(pool.pool).await;

        let mut app = test::init_service(App::new().data(storage.clone()).route(
            "/print/jobs/{id}",
            web::get().to(super::get_print_job_by_id),
        ))
        .await;

        let req = test::TestRequest::get()
            .uri("/print/jobs/1")
            .app_data("nonsense")
            .header("content-type", "text/plain")
            .to_request();
        let resp = test::call_service(&mut app, req).await;
        assert!(resp.status().is_success());
    }
}
