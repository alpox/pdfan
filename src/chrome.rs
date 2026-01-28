use std::time::Duration;
use std::{ops::Deref, sync::Arc};

use chromiumoxide::{
    Page,
    browser::{Browser, BrowserConfig},
    cdp::browser_protocol::page::PrintToPdfParams,
    page::MediaTypeParams,
};
use color_eyre::eyre::{Context, Result, eyre};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::wait::{setup_custom_event_wait, wait_for_network_idle};
use crate::worker::{Task, WorkerPool};

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
    #[serde(default)]
    print_background: bool,
    #[serde(default)]
    landscape: bool,
    margin_top: Option<f64>,
    margin_right: Option<f64>,
    margin_bottom: Option<f64>,
    margin_left: Option<f64>,
    #[serde(default)]
    display_header_footer: bool,
    header_template: Option<String>,
    footer_template: Option<String>,
    wait_for_resources: Option<bool>,
    #[serde(default)]
    wait_for_event: bool,
}

pub trait PdfDriver {
    type Payload;
    async fn pdf(&self, payload: Self::Payload) -> Result<Vec<u8>>;
}

/// Shared browser instance with its handler task
struct SharedBrowser {
    browser: Arc<Browser>,
    _handler_handle: JoinHandle<()>,
}

impl SharedBrowser {
    async fn launch() -> Result<Self> {
        let config = BrowserConfig::builder()
            .arg("--headless")
            .arg("--no-sandbox")
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .build()
            .map_err(|e| eyre!("Failed to build browser config: {}", e))?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .wrap_err("Failed to launch browser")?;

        // Spawn handler task - must run continuously for CDP communication
        let handler_handle = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    eprintln!("Browser handler error: {:?}", e);
                }
            }
        });

        Ok(Self {
            browser: Arc::new(browser),
            _handler_handle: handler_handle,
        })
    }

    fn browser(&self) -> Arc<Browser> {
        Arc::clone(&self.browser)
    }
}

/// Worker context holding a reusable page
pub struct ChromeTaskCtx {
    browser: Arc<Browser>,
    page: Page,
}

impl ChromeTaskCtx {
    async fn new(browser: Arc<Browser>) -> Result<Self> {
        let page = browser
            .new_page("about:blank")
            .await
            .wrap_err("Failed to create new page")?;

        Ok(Self { browser, page })
    }

    /// Recreate the page if it becomes unusable
    async fn recreate_page(&mut self) -> Result<()> {
        // Create fresh page (old page will be dropped, which closes it)
        self.page = self
            .browser
            .new_page("about:blank")
            .await
            .wrap_err("Failed to recreate page")?;

        Ok(())
    }
}

struct ChromeTask {
    payload: ChromeDriverPdfPayload,
}

impl ChromeTask {
    pub fn new(payload: ChromeDriverPdfPayload) -> Self {
        Self { payload }
    }

    async fn process_inner(&self, ctx: &mut ChromeTaskCtx) -> Result<Vec<u8>> {
        let p = &self.payload;

        if let Some(media) = &p.media {
            ctx.page
                .emulate_media_type(match media.deref() {
                    "null" => MediaTypeParams::Null,
                    "screen" => MediaTypeParams::Screen,
                    "print" => MediaTypeParams::Print,
                    _ => MediaTypeParams::Null,
                })
                .await?;
        }

        // Load content - set_content for HTML (fast!), goto for URLs
        if let Some(html) = &p.html {
            ctx.page
                .set_content(html)
                .await
                .wrap_err("Failed to set HTML content")?;
        } else if let Some(url) = &p.url {
            if p.wait_for_event {
                let wait_future = setup_custom_event_wait(&ctx.page).await?;
                ctx.page
                    .goto(url)
                    .await
                    .wrap_err("Failed to navigate to URL")?;
                wait_future.await?;
            } else {
                ctx.page
                    .goto(url)
                    .await
                    .wrap_err("Failed to navigate to URL")?;

                match p.wait_for_resources {
                    Some(true) => {
                        wait_for_network_idle(&ctx.page, crate::wait::NetworkIdleKind::Idle0)
                            .await?
                    }
                    Some(false) => {
                        wait_for_network_idle(&ctx.page, crate::wait::NetworkIdleKind::Idle2)
                            .await?
                    }
                    None => {}
                }
            }
        } else {
            return Err(eyre!("Either url or html must be provided"));
        }

        // Build PDF parameters
        let display_header_footer = p.header_template.is_some() || p.footer_template.is_some();

        let mut pdf_params = PrintToPdfParams::builder()
            .print_background(p.print_background)
            .landscape(p.landscape)
            .display_header_footer(display_header_footer)
            .margin_top(p.margin_top.unwrap_or(0.0))
            .margin_right(p.margin_right.unwrap_or(0.0))
            .margin_bottom(p.margin_bottom.unwrap_or(0.0))
            .margin_left(p.margin_left.unwrap_or(0.0));

        // Handle dimensions
        if let (Some(_w), Some(_h)) = (&p.width, &p.height) {
            // TODO: Parse width/height strings to f64 if they include units
            // For now, use format-based dimensions
            let (w, h) = format_to_inches(p.format.as_deref().unwrap_or("A4"));
            pdf_params = pdf_params.paper_width(w).paper_height(h);
        } else {
            let (w, h) = format_to_inches(p.format.as_deref().unwrap_or("A4"));
            pdf_params = pdf_params.paper_width(w).paper_height(h);
        }

        // Optional fields
        if let Some(ranges) = &p.print_range {
            pdf_params = pdf_params.page_ranges(ranges.clone());
        }
        if let Some(header) = &p.header_template {
            pdf_params = pdf_params.header_template(header.clone());
        }
        if let Some(footer) = &p.footer_template {
            pdf_params = pdf_params.footer_template(footer.clone());
        }

        // Generate PDF
        let pdf_bytes = ctx
            .page
            .pdf(pdf_params.build())
            .await
            .wrap_err("Failed to generate PDF")?;

        Ok(pdf_bytes)
    }
}

impl Task<ChromeTaskCtx> for ChromeTask {
    type Result = Result<Vec<u8>>;

    async fn process(&self, ctx: &mut ChromeTaskCtx) -> Self::Result {
        match self.process_inner(ctx).await {
            Ok(result) => Ok(result),
            Err(e) => {
                // Attempt recovery by recreating page
                if ctx.recreate_page().await.is_ok() {
                    // Retry once with fresh page
                    self.process_inner(ctx).await
                } else {
                    Err(e)
                }
            }
        }
    }
}

pub struct ChromeDriver {
    pool: WorkerPool<ChromeTaskCtx, ChromeTask>,
    _shared_browser: SharedBrowser,
    task_timeout: Duration,
}

impl ChromeDriver {
    pub async fn new(task_timeout: Duration) -> Result<Self> {
        let shared_browser = SharedBrowser::launch().await?;
        let browser = shared_browser.browser();

        let pool = WorkerPool::new(30, 4, move || {
            let browser = Arc::clone(&browser);
            async move { ChromeTaskCtx::new(browser).await }
        });

        Ok(Self {
            pool,
            _shared_browser: shared_browser,
            task_timeout,
        })
    }
}

impl PdfDriver for ChromeDriver {
    type Payload = ChromeDriverPdfPayload;

    async fn pdf(&self, payload: Self::Payload) -> Result<Vec<u8>> {
        let task = ChromeTask::new(payload);
        self.pool.queue(task, self.task_timeout).await.flatten()
    }
}
