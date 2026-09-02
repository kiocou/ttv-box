//! Live smoke test for the adult metadata pipeline
//! (JavBus -> JavDB -> Avmoo -> JavLibrary -> Jav321 -> sehuatang).
//!
//! Usage:
//!   cargo run --example jav_smoke -- [CODE_OR_FILENAME]
//!   cargo run --example jav_smoke -- --avmoo CODE
//!   cargo run --example jav_smoke -- --javbus CODE
//!   cargo run --example jav_smoke -- --javdb CODE
//!   cargo run --example jav_smoke -- --javlibrary CODE
//!   cargo run --example jav_smoke -- --jav321 CODE
//!   cargo run --example jav_smoke -- --sehuatang CODE
//!
//! Exercises the same path as the library scraper: file-name code
//! extraction, then multi-source lookup with the shared client. On a match it
//! also downloads the cover to prove the atomic-write path.

use ttv_backend::adult;
use ttv_backend::error::AppError;

#[tokio::main]
async fn main() {
    let mut only: Option<&str> = None;
    let mut input = "ABP-356".to_string();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--avmoo" => only = Some("avmoo"),
            "--javbus" => only = Some("javbus"),
            "--javdb" => only = Some("javdb"),
            "--javlibrary" => only = Some("javlibrary"),
            "--jav321" => only = Some("jav321"),
            "--sehuatang" => only = Some("sehuatang"),
            other => input = other.to_string(),
        }
    }

    let extracted = adult::code::extract_codes_from_name(&input);
    let candidates = if extracted.is_empty() {
        vec![input.clone()]
    } else {
        extracted
    };
    println!("input:      {input}");
    println!("candidates: {candidates:?}");
    println!(
        "mode:       {}",
        only.unwrap_or("javbus->javdb->avmoo->javlibrary->jav321->sehuatang")
    );

    let client = adult::build_client().expect("client");

    if let Some(source) = only {
        let code = &candidates[0];
        let result = match source {
            "avmoo" => adult::avmoo::lookup(&client, code).await,
            "javdb" => adult::javdb::lookup(&client, code).await,
            "javlibrary" => adult::javlibrary::lookup(&client, code).await,
            "jav321" => adult::jav321::lookup(&client, code).await,
            "sehuatang" => adult::sehuatang::lookup(code).await,
            _ => adult::javbus::lookup(&client, code).await,
        };
        report(&client, result).await;
        return;
    }

    report(&client, adult::lookup_jav(&client, &candidates).await).await;
}

async fn report(client: &reqwest::Client, result: Result<Option<adult::JavMatch>, AppError>) {
    match result {
        Ok(Some(matched)) => {
            println!("MATCH via {}", matched.provider);
            println!("  code:     {}", matched.code);
            println!("  title:    {}", matched.title);
            println!("  series:   {:?}", matched.series);
            println!("  studio:   {:?}", matched.studio);
            println!("  director: {:?}", matched.director);
            println!("  label:    {:?}", matched.label);
            println!("  release:  {:?}", matched.release_date);
            println!("  duration: {:?}", matched.duration_min);
            println!("  rating:   {:?}", matched.rating);
            println!("  tags:     {:?}", matched.tags);
            println!("  actors:   {:?}", matched.actors);
            println!("  summary:  {:?}", matched.summary);
            println!("  cover:    {:?}", matched.cover_url);

            if let Some(url) = matched.cover_url.as_deref() {
                let dir = std::env::temp_dir().join("ttv-jav-smoke-covers");
                std::fs::create_dir_all(&dir).ok();
                let referer = adult::cover::referer_for_provider(&matched.provider);
                match adult::cover::download_cover(client, &dir, &matched.code, url, false, referer)
                    .await
                {
                    Ok(path) => {
                        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        println!("  downloaded cover -> {} ({} bytes)", path.display(), bytes);
                    }
                    Err(error) => println!("  cover download failed: {error}"),
                }
            }
        }
        Ok(None) => println!("NOT FOUND (definitive 404 from at least one source)"),
        Err(error) => println!("TRANSIENT ERROR: {error}"),
    }
}
