use super::super::*;

pub(in crate::http) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
