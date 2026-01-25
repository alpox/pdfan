// use axum::{
//     routing::get,
//     Router,
// };
//
// #[tokio::main]
// async fn main() {
//     // build our application with a single route
//     let app = Router::new().route("/", get(|| async { "Hello, World!" }));
//
//     // run our app with hyper, listening globally on port 3000
//     let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
//     axum::serve(listener, app).await.unwrap();
// }

use fantoccini::{ClientBuilder, Locator, wd::PrintConfiguration};
use serde::{Deserialize, Serialize};

use crate::driver::{ChromeDriver, Supervisor};

pub mod driver;
pub mod worker;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChromeDriverPdfPayload {
    url: String,
    html: String,
    title: String,
    author: String,
    media: String,
    format: String,
    width: String,
    height: String,
    print_range: String,
    print_background: bool,
    landscape: bool,
    margin_top: u32,
    margin_right: u32,
    margin_bottom: u32,
    margin_left: u32,
    display_header_footer: bool,
    header_template: String,
    footer_template: String,
    wait_for_resources: Option<bool>,
    wait_for_event: bool,
}

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

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let supervisor = Supervisor::new();
    supervisor.run(ChromeDriver);

    let c = ClientBuilder::native()
        .connect("http://localhost:4444")
        .await
        .expect("failed to connect to WebDriver");

    // first, go to the Wikipedia page for Foobar
    c.goto("https://en.wikipedia.org/wiki/Foobar").await?;
    let url = c.current_url().await?;
    assert_eq!(url.as_ref(), "https://en.wikipedia.org/wiki/Foobar");

    let pdf = c.print(PrintConfiguration::default()).await?;
    std::fs::write("test.pdf", pdf)?;

    // click "Foo (disambiguation)"
    c.find(Locator::Css(".mw-disambig")).await?.click().await?;

    // click "Foo Lake"
    c.find(Locator::LinkText("Foo Lake")).await?.click().await?;

    let url = c.current_url().await?;
    assert_eq!(url.as_ref(), "https://en.wikipedia.org/wiki/Foo_Lake");

    c.close().await?;

    supervisor.stop().await;

    Ok(())
}
