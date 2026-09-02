//! End-to-end smoke test for the Red Fruit motion-comic path.
//!
//! Verifies the public chart, the shared detail endpoint, and the first
//! browser-playable episode without touching the application database.

use ttv_backend::comic_drama::{
    comic_drama_detail, comic_drama_play, comic_drama_stream, ComicDramaStreamInput,
};
use ttv_backend::short_drama::{ShortDramaDetailInput, ShortDramaPlayInput};

#[tokio::main]
async fn main() {
    let page = comic_drama_stream(ComicDramaStreamInput {
        cursor: None,
        facet: None,
    })
    .await
    .unwrap_or_else(|error| panic!("[stream] failed: {error}"));

    assert!(!page.items.is_empty(), "[stream] returned no comic dramas");
    let card = &page.items[0];
    assert!(!card.id.is_empty(), "[stream] missing series id");
    assert!(!card.title.is_empty(), "[stream] missing title");
    assert!(!card.cover_url.is_empty(), "[stream] missing cover");
    println!(
        "[stream] {} | {} | next={:?}",
        card.title, card.category, page.next_cursor
    );

    let detail = comic_drama_detail(ShortDramaDetailInput {
        series_id: card.id.clone(),
    })
    .await
    .unwrap_or_else(|error| panic!("[detail] failed: {error}"));
    assert!(!detail.vids.is_empty(), "[detail] missing episodes");
    println!(
        "[detail] {} | {} | episodes={} playable={}",
        detail.title,
        detail.episodes_text,
        detail.vids.len(),
        detail.playable_episodes
    );

    let playback = comic_drama_play(ShortDramaPlayInput {
        series_id: card.id.clone(),
        vid: detail.vids[0].clone(),
    })
    .await
    .unwrap_or_else(|error| panic!("[play] failed: {error}"));
    assert!(!playback.url.is_empty(), "[play] missing media URL");
    println!(
        "[play] episode={} total={} url_len={}",
        playback.episode,
        playback.total_episodes,
        playback.url.len()
    );
}
