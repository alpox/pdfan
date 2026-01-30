use std::collections::HashSet;
use std::time::Duration;

use chromiumoxide::{
    Page,
    cdp::browser_protocol::network::{
        EnableParams as NetworkEnableParams, EventLoadingFailed, EventLoadingFinished,
        EventRequestWillBeSent, SetCacheDisabledParams,
    },
    cdp::browser_protocol::page::EventDomContentEventFired,
    cdp::js_protocol::runtime::EventBindingCalled,
};
use color_eyre::eyre::Result;
use futures::StreamExt;
use tokio::sync::mpsc;

/// Disable browser cache for consistent performance
pub async fn disable_cache(page: &Page) -> Result<()> {
    page.execute(NetworkEnableParams::default()).await?;
    page.execute(SetCacheDisabledParams::new(true)).await?;
    Ok(())
}

/// Set up a listener for DOMContentLoaded event.
///
/// Must be called BEFORE navigation (goto). Returns a future that resolves when
/// DOMContentLoaded fires.
pub async fn setup_dom_content_loaded_wait(
    page: &Page,
) -> Result<impl std::future::Future<Output = Result<()>>> {
    let mut dom_events = page.event_listener::<EventDomContentEventFired>().await?;

    Ok(async move {
        dom_events.next().await;
        Ok(())
    })
}

/// Network idle detection strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkIdleKind {
    /// Wait until no network connections for 500ms (puppeteer's networkidle0)
    Idle0,
    /// Wait until no more than 2 network connections for 500ms (puppeteer's networkidle2)
    Idle2,
}

enum NetworkEvent {
    RequestStarted(String),
    RequestFinished(String),
}

/// Wait for network to become idle.
///
/// This replicates Puppeteer's `waitUntil: 'networkidle0'` and `waitUntil: 'networkidle2'` options.
pub async fn wait_for_network_idle(page: &Page, kind: NetworkIdleKind) -> Result<()> {
    const IDLE_TIMEOUT: Duration = Duration::from_millis(500);
    let max_connections = match kind {
        NetworkIdleKind::Idle0 => 0,
        NetworkIdleKind::Idle2 => 2,
    };

    // Enable network tracking
    page.execute(NetworkEnableParams::default()).await?;

    let mut request_events = page.event_listener::<EventRequestWillBeSent>().await?;
    let mut finished_events = page.event_listener::<EventLoadingFinished>().await?;
    let mut failed_events = page.event_listener::<EventLoadingFailed>().await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<NetworkEvent>();

    // Spawn event collector tasks
    let tx1 = tx.clone();
    let request_task = tokio::spawn(async move {
        while let Some(event) = request_events.next().await {
            let _ = tx1.send(NetworkEvent::RequestStarted(event.request_id.inner().to_string()));
        }
    });

    let tx2 = tx.clone();
    let finished_task = tokio::spawn(async move {
        while let Some(event) = finished_events.next().await {
            let _ = tx2.send(NetworkEvent::RequestFinished(event.request_id.inner().to_string()));
        }
    });

    let tx3 = tx.clone();
    let failed_task = tokio::spawn(async move {
        while let Some(event) = failed_events.next().await {
            let _ = tx3.send(NetworkEvent::RequestFinished(event.request_id.inner().to_string()));
        }
    });

    drop(tx); // Drop original sender so channel closes when tasks complete

    let mut pending_requests: HashSet<String> = HashSet::new();
    let mut idle_since: Option<tokio::time::Instant> = None;

    loop {
        let timeout = tokio::time::sleep(Duration::from_millis(100));

        tokio::select! {
            event = rx.recv() => {
                match event {
                    Some(NetworkEvent::RequestStarted(id)) => {
                        pending_requests.insert(id);
                        idle_since = None;
                    }
                    Some(NetworkEvent::RequestFinished(id)) => {
                        pending_requests.remove(&id);
                    }
                    None => break, // Channel closed
                }
            }
            _ = timeout => {
                // Check if we're idle
                if pending_requests.len() <= max_connections {
                    match idle_since {
                        Some(since) if since.elapsed() >= IDLE_TIMEOUT => break,
                        None => idle_since = Some(tokio::time::Instant::now()),
                        _ => {}
                    }
                } else {
                    idle_since = None;
                }
            }
        }
    }

    request_task.abort();
    finished_task.abort();
    failed_task.abort();

    Ok(())
}

/// Wait for a custom event triggered by calling `window.finishRendering()`.
///
/// This sets up a binding so that the page can signal when it's ready for PDF generation.
/// The page should dispatch a 'prerender-trigger' event, or call `window.finishRendering()` directly.
///
/// Must be called BEFORE navigation (goto).
pub async fn setup_custom_event_wait(page: &Page) -> Result<impl std::future::Future<Output = Result<()>>> {
    // Expose the finishRendering function
    page.expose_function("finishRendering", "function() {}").await?;

    // Set up the event listener before navigation
    page.evaluate_on_new_document(
        r#"
        window.prerender = true;
        window.document.addEventListener('prerender-trigger', () => {
            window.finishRendering();
        });
        "#.to_string()
    ).await?;

    let mut binding_events = page.event_listener::<EventBindingCalled>().await?;

    Ok(async move {
        while let Some(event) = binding_events.next().await {
            if event.name == "finishRendering" {
                break;
            }
        }
        Ok(())
    })
}
