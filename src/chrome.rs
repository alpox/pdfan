use std::time::Duration;

use base64::prelude::*;
use color_eyre::eyre::{eyre, Context, Result};
use fantoccini::{Client, ClientBuilder, wd::{Capabilities, WebDriverCompatibleCommand}};
use http::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::worker::{Task, WorkerPool};

/// A CDP command that can be issued through fantoccini's WebDriver connection.
#[derive(Debug)]
struct CdpCommand {
    cmd: String,
    params: Value,
}

impl CdpCommand {
    fn new(cmd: impl Into<String>, params: Value) -> Self {
        Self { cmd: cmd.into(), params }
    }
}

impl WebDriverCompatibleCommand for CdpCommand {
    fn endpoint(
        &self,
        base_url: &url::Url,
        session_id: Option<&str>,
    ) -> std::result::Result<url::Url, url::ParseError> {
        base_url.join(&format!("session/{}/goog/cdp/execute", session_id.unwrap()))
    }

    fn method_and_body(&self, _request_url: &url::Url) -> (Method, Option<String>) {
        let body = json!({ "cmd": &self.cmd, "params": &self.params });
        (Method::POST, Some(body.to_string()))
    }

    fn is_new_session(&self) -> bool {
        false
    }
}

fn format_to_inches(format: &str) -> (f64, f64) {
    match format.to_uppercase().as_str() {
        "LETTER" => (8.5, 11.0),
        "LEGAL" => (8.5, 14.0),
        "TABLOID" => (11.0, 17.0),
        "LEDGER" => (17.0, 11.0),
        "A0" => (33.1, 46.8),
        "A1" => (23.4, 33.1),
        "A2" => (16.5, 23.4),
        "A3" => (11.7, 16.5),
        "A4" => (8.27, 11.7),
        "A5" => (5.83, 8.27),
        "A6" => (4.13, 5.83),
        _ => (8.27, 11.7), // Default to A4
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChromeDriverPdfPayload {
    url: Option<String>,
    html: Option<String>,
    title: Option<String>,
    author: Option<String>,
    media: Option<String>,
    format: Option<String>,
    width: Option<String>,
    height: Option<String>,
    print_range: Option<String>,
    print_background: bool,
    landscape: bool,
    margin_top: Option<f64>,
    margin_right: Option<f64>,
    margin_bottom: Option<f64>,
    margin_left: Option<f64>,
    display_header_footer: bool,
    header_template: Option<String>,
    footer_template: Option<String>,
    wait_for_resources: Option<bool>,
    wait_for_event: bool,
}

pub trait PdfDriver {
    type Payload;
    async fn pdf(&self, payload: Self::Payload) -> Result<Vec<u8>>;
}

struct ChromeTaskCtx {
    client: Client,
}

struct ChromeTask {
    payload: ChromeDriverPdfPayload,
}

impl ChromeTask {
    pub fn new(payload: ChromeDriverPdfPayload) -> Self {
        Self { payload }
    }
}

impl Task<ChromeTaskCtx> for ChromeTask {
    type Result = Result<Vec<u8>>;

    async fn process(&self, ctx: &mut ChromeTaskCtx) -> Self::Result {
        let c = &ctx.client;
        let p = &self.payload;

        // Navigate to URL or load HTML via data URL
        if let Some(url) = &p.url {
            c.goto(url).await?;
        } else if let Some(html) = &p.html {
            let encoded = BASE64_STANDARD.encode(html);
            c.goto(&format!("data:text/html;base64,{encoded}")).await?;
        }

        // Determine if we should display header/footer (matches TypeScript logic)
        let display_header_footer = p.header_template.is_some() || p.footer_template.is_some();

        // Build CDP params matching the TypeScript implementation
        let mut params = json!({
            "printBackground": p.print_background,
            "landscape": p.landscape,
            "displayHeaderFooter": display_header_footer,
            "marginTop": p.margin_top.unwrap_or(0.0),
            "marginRight": p.margin_right.unwrap_or(0.0),
            "marginBottom": p.margin_bottom.unwrap_or(0.0),
            "marginLeft": p.margin_left.unwrap_or(0.0),
        });

        // Add optional fields
        if let Some(ranges) = &p.print_range {
            params["pageRanges"] = json!(ranges);
        }
        if let Some(header) = &p.header_template {
            params["headerTemplate"] = json!(header);
        }
        if let Some(footer) = &p.footer_template {
            params["footerTemplate"] = json!(footer);
        }

        // Handle dimensions: use width/height if both provided, otherwise use format
        if let (Some(w), Some(h)) = (&p.width, &p.height) {
            params["paperWidth"] = json!(w);
            params["paperHeight"] = json!(h);
        } else {
            let (w, h) = format_to_inches(p.format.as_deref().unwrap_or("A4"));
            params["paperWidth"] = json!(w);
            params["paperHeight"] = json!(h);
        }

        // Execute CDP command
        let result = c
            .issue_cmd(CdpCommand::new("Page.printToPDF", params))
            .await
            .wrap_err("failed to execute Page.printToPDF")?;

        // Decode base64 PDF data from response
        let pdf_base64 = result["data"]
            .as_str()
            .ok_or_else(|| eyre!("No PDF data in response"))?;

        BASE64_STANDARD
            .decode(pdf_base64)
            .wrap_err("failed to decode PDF base64")
    }
}

pub struct ChromeDriver {
    pool: WorkerPool<ChromeTaskCtx, ChromeTask>,
    task_timeout: Duration,
}

impl ChromeDriver {
    pub fn new(task_timeout: Duration) -> Self {
        let pool = WorkerPool::new(30, 10, || async {
            let mut caps = Capabilities::new();

            caps.insert(
                "goog:chromeOptions".to_string(),
                json!({
                    "args": ["--headless", "--disable-gpu", "--no-sandbox"]
                }),
            );

            let client = ClientBuilder::native()
                .capabilities(caps)
                .connect("http://localhost:4444")
                .await
                .wrap_err("failed to connect to WebDriver")?;

            Ok(ChromeTaskCtx { client })
        });

        Self { pool, task_timeout }
    }
}

impl PdfDriver for ChromeDriver {
    type Payload = ChromeDriverPdfPayload;

    async fn pdf(&self, payload: Self::Payload) -> Result<Vec<u8>> {
        let task = ChromeTask::new(payload);
        self.pool.queue(task, self.task_timeout).await.flatten()
    }
}
