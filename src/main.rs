use color_eyre::eyre::Result;
use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};

use crate::chrome::{ChromeDriver, ChromeDriverPdfPayload, PdfDriver};

pub mod chrome;
pub mod wait;
pub mod worker;

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
async fn main() -> Result<()> {
    color_eyre::install()?;

    let chrome_driver = Arc::new(
        ChromeDriver::new(Duration::from_secs(30))
            .await
            .expect("Failed to initialize Chrome driver"),
    );

    // build our application with a single route
    let app = Router::new()
        .route("/api/convert", post(handle_pdf))
        .with_state(chrome_driver);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("Listening on port 3000");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn handle_pdf(
    State(driver): State<Arc<ChromeDriver>>,
    Json(payload): Json<ChromeDriverPdfPayload>,
) -> Result<Vec<u8>, AppError> {
    Ok(driver.pdf(payload).await?)
}
