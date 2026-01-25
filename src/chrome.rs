use std::time::Duration;

use color_eyre::eyre::{Context, Result};
use fantoccini::{Client, ClientBuilder, wd::{Capabilities, PrintConfiguration}};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::worker::{Task, WorkerPool};

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
    margin_top: Option<u32>,
    margin_right: Option<u32>,
    margin_bottom: Option<u32>,
    margin_left: Option<u32>,
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

        // first, go to the Wikipedia page for Foobar
        c.goto("https://en.wikipedia.org/wiki/Foobar").await?;
        let url = c.current_url().await?;
        assert_eq!(url.as_ref(), "https://en.wikipedia.org/wiki/Foobar");

        c.print(PrintConfiguration::default())
            .await
            .wrap_err("failed to print pdf")
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

            caps.insert("goog:chromeOptions".to_string(), json!({
                "args": ["--headless", "--disable-gpu", "--no-sandbox"]
            }));

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
