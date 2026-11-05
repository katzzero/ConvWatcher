//! Static test for the dashboard HTML/JS.
//!
//! Asserts that the embedded dashboard contains the auth/UX features without
//! needing a browser: Authorization header, sessionStorage, aria-live, w.name,
//! /logs/errors, refresh controls, :focus-visible, FFmpeg output tab, progress
//! bar and 1s/2s refresh options.

#[test]
fn dashboard_contains_auth_and_ux_features() {
    let html = include_str!("../src/health/dashboard.html");

    // Auth: fetchJson/fetchText send Authorization; password kept in sessionStorage.
    assert!(
        html.contains("Authorization"),
        "missing Authorization header"
    );
    assert!(
        html.contains("sessionStorage"),
        "missing sessionStorage password storage"
    );
    assert!(html.contains("Bearer"), "missing Bearer construction");
    assert!(
        html.contains("cw_auth_password"),
        "missing cw_auth_password sessionStorage key"
    );
    assert!(
        html.contains("Password"),
        "missing Password label/placeholder/button"
    );

    // Accessibility: aria-live alerts/stats and focus-visible styles.
    assert!(html.contains("aria-live"), "missing aria-live");
    assert!(
        html.contains(":focus-visible"),
        "missing :focus-visible focus style"
    );

    // U1: watcher card uses w.name (title) and watch_folder (detail).
    assert!(html.contains("w.name"), "missing w.name watcher title");

    // U6: app/error log selector with /logs/errors endpoint.
    assert!(
        html.contains("/logs/errors"),
        "missing /logs/errors log endpoint"
    );

    // U7: controllable refresh (1s/2s/5s/10s/30s/Off) + Refresh now.
    assert!(
        html.contains("refreshInterval"),
        "missing refresh interval control"
    );
    assert!(html.contains("Off"), "missing Off refresh option");
    assert!(html.contains("Refresh now"), "missing Refresh now button");
    assert!(html.contains("value=\"1000\""), "missing 1s refresh option");
    assert!(html.contains("value=\"2000\""), "missing 2s refresh option");

    // U8: alert role / aria-live for errors.
    assert!(html.contains(r#"role="alert""#), "missing role=alert");

    // FFmpeg output log selector option + /api/ffmpeg endpoint.
    assert!(
        html.contains("FFmpeg output"),
        "missing FFmpeg output log option"
    );
    assert!(
        html.contains("/api/ffmpeg"),
        "missing /api/ffmpeg log endpoint"
    );

    // Progress bar / speed / ETA rendering in queue section.
    assert!(
        html.contains("progressInfo"),
        "missing progress info container"
    );
    assert!(html.contains("ETA"), "missing ETA label");
    assert!(html.contains("Speed:"), "missing Speed label");
    assert!(
        html.contains("processing (no duration)"),
        "missing indeterminate (spinner) state"
    );

    // sessionStorage persists log selection.
    assert!(
        html.contains("cw_log_type"),
        "missing cw_log_type sessionStorage key"
    );
}
