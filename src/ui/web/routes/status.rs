use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::ui::web::state::AppState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) async fn api_status(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "version": VERSION,
        "prewarm_in_progress": crate::prewarm_state::is_prewarm_in_progress(),
        "save_dir": state.library_root.to_string_lossy(),
        "bind_addr": state.bind.to_string(),
        "locked": state.auth.is_some(),
        "config": {
            "old_cli": state.config_view.old_cli,
            "use_official_api": state.config_view.use_official_api,
            "save_path": state.config_view.save_path,
            "api_endpoints_len": state.config_view.api_endpoints_len,
        }
    }))
}
