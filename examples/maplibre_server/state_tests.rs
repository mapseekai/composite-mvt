use super::*;

#[tokio::test]
async fn configuration_updates_do_not_wait_for_in_flight_tiles() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let upstream = axum::Router::new().fallback({
        let started = started.clone();
        let release = release.clone();
        move |uri: axum::http::Uri| {
            let started = started.clone();
            let release = release.clone();
            async move {
                if uri.path().starts_with("/old/") {
                    started.notify_one();
                    release.notified().await;
                    "old tile"
                } else {
                    "new tile"
                }
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let state = Arc::new(AppState::new());
    state
        .configure(&format!(
            "http://{address}/old/{{z}}/{{x}}/{{y}}\told\tnone"
        ))
        .await
        .unwrap();
    let old_request = tokio::spawn({
        let state = state.clone();
        async move { state.compose_tile("0", "0", "0").await }
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();

    let new_config = format!("http://{address}/new/{{z}}/{{x}}/{{y}}\tnew\tnone");
    tokio::time::timeout(Duration::from_secs(2), state.configure(&new_config))
        .await
        .expect("configuration update waited for the old upstream")
        .unwrap();
    let new_tile = tokio::time::timeout(Duration::from_secs(2), state.compose_tile("0", "0", "0"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_tile, b"new tile");

    release.notify_one();
    let old_tile = tokio::time::timeout(Duration::from_secs(2), old_request)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(old_tile, b"old tile");
    server.abort();
}
