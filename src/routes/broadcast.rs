//! One `WsHub` broadcast per entity mutation, called from all three edges
//! that can perform it (the REST API, the MCP server, and the AI
//! assistant's built-in tools) so an admin tab stays live regardless of
//! which surface made the change (issue #25). Also hosts the response DTOs
//! (`PageSummary`, `FileSummary`) that double as both the HTTP response body
//! and the WS payload, so there's exactly one place each entity's public
//! shape is assembled.

use serde_json::json;

use crate::entity::{gallery, page, tag};
use crate::repo::files as files_repo;
use crate::routes::ws::{Topic, WsHub};

#[derive(serde::Serialize)]
pub struct PageSummary {
    pub id: i32,
    pub path: String,
    pub summary: Option<String>,
    pub tag_ids: Vec<i32>,
    pub private: bool,
    pub created_at: String,
    pub modified_at: String,
}

impl From<&page::Model> for PageSummary {
    fn from(p: &page::Model) -> Self {
        Self {
            id: p.id,
            path: p.path.clone(),
            summary: p.summary.clone(),
            tag_ids: p.tag_ids.clone(),
            private: p.private,
            created_at: p.created_at.to_string(),
            modified_at: p.modified_at.to_string(),
        }
    }
}

pub fn page_created(hub: &WsHub, page: &page::Model) -> PageSummary {
    let summary = PageSummary::from(page);
    hub.broadcast_serialized(Topic::Pages, "created", &summary);
    summary
}

pub fn page_updated(hub: &WsHub, page: &page::Model) -> PageSummary {
    let summary = PageSummary::from(page);
    hub.broadcast_serialized(Topic::Pages, "updated", &summary);
    summary
}

pub fn page_deleted(hub: &WsHub, id: i32) {
    hub.broadcast_event(Topic::Pages, "deleted", json!({ "id": id }));
}

#[derive(serde::Serialize)]
pub struct FileSummary {
    pub id: i32,
    pub hash: String,
    pub path: String,
    pub title: String,
    pub description: Option<String>,
    pub mimetype: String,
    pub size_bytes: i64,
    pub has_thumbnail: bool,
    pub created_at: String,
}

impl FileSummary {
    pub fn new(model: &crate::entity::file::Model, has_thumbnail: bool) -> Self {
        Self {
            id: model.id,
            hash: model.hash.clone(),
            title: files_repo::title_from_path(&model.path),
            path: model.path.clone(),
            description: model.description.clone(),
            mimetype: model.mimetype.clone(),
            size_bytes: model.size_bytes,
            has_thumbnail,
            created_at: model.created_at.to_string(),
        }
    }
}

pub fn file_created(
    hub: &WsHub,
    model: &crate::entity::file::Model,
    has_thumbnail: bool,
) -> FileSummary {
    let summary = FileSummary::new(model, has_thumbnail);
    hub.broadcast_serialized(Topic::Files, "created", &summary);
    summary
}

pub fn file_updated(
    hub: &WsHub,
    model: &crate::entity::file::Model,
    has_thumbnail: bool,
) -> FileSummary {
    let summary = FileSummary::new(model, has_thumbnail);
    hub.broadcast_serialized(Topic::Files, "updated", &summary);
    summary
}

pub fn file_deleted(hub: &WsHub, id: i32) {
    hub.broadcast_event(Topic::Files, "deleted", json!({ "id": id }));
}

pub fn gallery_created(hub: &WsHub, gallery: &gallery::Model) {
    hub.broadcast_serialized(Topic::Galleries, "created", gallery);
}

pub fn gallery_updated(hub: &WsHub, gallery: &gallery::Model) {
    hub.broadcast_serialized(Topic::Galleries, "updated", gallery);
}

pub fn gallery_deleted(hub: &WsHub, id: i32) {
    hub.broadcast_event(Topic::Galleries, "deleted", json!({ "id": id }));
}

pub fn tag_created(hub: &WsHub, tag: &tag::Model) {
    hub.broadcast_serialized(Topic::Tags, "created", tag);
}

pub fn tag_updated(hub: &WsHub, tag: &tag::Model) {
    hub.broadcast_serialized(Topic::Tags, "updated", tag);
}

pub fn tag_deleted(hub: &WsHub, id: i32) {
    hub.broadcast_event(Topic::Tags, "deleted", json!({ "id": id }));
}
