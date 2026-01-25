use color_eyre::eyre::Result;
use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};

use serde::{Deserialize, Serialize};

use crate::chrome::{ChromeDriver, ChromeDriverPdfPayload, PdfDriver};

pub mod chrome;
pub mod worker;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TypstDriverPdfPayload {
    content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", untagged)]
enum PdfPayload {
    ChromeDriver(Box<ChromeDriverPdfPayload>),
    Typst(Box<TypstDriverPdfPayload>),
}

struct AppError(color_eyre::eyre::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
    }
}

impl<E: Into<color_eyre::eyre::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[tokio::main]
async fn main() {
    let chrome_driver = Arc::new(ChromeDriver::new(Duration::from_secs(30)));

    // build our application with a single route
    let app = Router::new()
        .route("/", get(handle_pdf))
        .with_state(chrome_driver);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handle_pdf(State(driver): State<Arc<ChromeDriver>>) -> Result<Vec<u8>, AppError> {
    Ok(driver.pdf(ChromeDriverPdfPayload::default()).await?)
}
