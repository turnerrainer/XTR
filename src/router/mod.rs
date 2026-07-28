//! Axum router. Routes (per DESIGN.md §8.2):
//! * `POST /:group/:service` — invoke a mapped X-Road service
//! * `GET /api` — auto-generated OpenAPI 3.1 (Phase G)
//! * `GET /health` — `{"status":"ok"}`
//!
//! Full implementation in Phase F.
