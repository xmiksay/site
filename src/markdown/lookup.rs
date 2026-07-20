//! Resolving `path`/`id`/`hash` directive arguments to DB rows (files,
//! galleries, pages).

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::file as file_entity;
use crate::entity::gallery as gallery_entity;
use crate::entity::page as page_entity;

use super::directives::Directive;

#[derive(Debug)]
pub(crate) enum FileLookup {
    Path(String),
    Id(i32),
    Hash(String),
}

pub(super) fn parse_file_lookup(d: &Directive, name: &str) -> Result<FileLookup, String> {
    let path = d.arg("path").filter(|s| !s.is_empty());
    let id = d.arg("id").filter(|s| !s.is_empty());
    let hash = d.arg("hash").filter(|s| !s.is_empty());

    let count = path.is_some() as u8 + id.is_some() as u8 + hash.is_some() as u8;
    match count {
        0 => Err(format!(
            "\n\n*[`<{name}>` requires `path`, `id`, or `hash`]*\n\n"
        )),
        1 => {
            if let Some(p) = path {
                Ok(FileLookup::Path(p.to_owned()))
            } else if let Some(i) = id {
                let n: i32 = i.parse().map_err(|_| {
                    format!("\n\n*[`<{name}>` got invalid `id` (expected integer)]*\n\n")
                })?;
                Ok(FileLookup::Id(n))
            } else {
                let h = hash.unwrap().to_ascii_lowercase();
                if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!(
                        "\n\n*[`<{name}>` got invalid `hash` (expected 64 hex chars)]*\n\n"
                    ));
                }
                Ok(FileLookup::Hash(h))
            }
        }
        _ => Err(format!(
            "\n\n*[`<{name}>` accepts only one of `path`, `id`, `hash`]*\n\n"
        )),
    }
}

pub(crate) async fn fetch_file(
    db: &DatabaseConnection,
    lookup: &FileLookup,
) -> Option<file_entity::Model> {
    match lookup {
        FileLookup::Id(id) => file_entity::Entity::find_by_id(*id)
            .one(db)
            .await
            .ok()
            .flatten(),
        FileLookup::Hash(h) => file_entity::Entity::find()
            .filter(file_entity::Column::Hash.eq(h.as_str()))
            .one(db)
            .await
            .ok()
            .flatten(),
        FileLookup::Path(p) => file_entity::Entity::find()
            .filter(file_entity::Column::Path.eq(crate::path_util::normalize(p)))
            .one(db)
            .await
            .ok()
            .flatten(),
    }
}

pub(super) fn lookup_label(lookup: &FileLookup) -> String {
    match lookup {
        FileLookup::Path(p) => p.clone(),
        FileLookup::Id(i) => i.to_string(),
        FileLookup::Hash(h) => h.clone(),
    }
}

#[derive(Debug)]
pub(super) enum GalleryLookup {
    Path(String),
    Id(i32),
}

pub(super) fn parse_gallery_lookup(d: &Directive) -> Result<GalleryLookup, String> {
    let path = d.arg("path").filter(|s| !s.is_empty());
    let id = d.arg("id").filter(|s| !s.is_empty());

    match (path, id) {
        (Some(p), None) => Ok(GalleryLookup::Path(p.to_owned())),
        (None, Some(i)) => {
            let n: i32 = i.parse().map_err(|_| {
                "\n\n*[`<gallery>` got invalid `id` (expected integer)]*\n\n".to_owned()
            })?;
            Ok(GalleryLookup::Id(n))
        }
        (Some(_), Some(_)) => {
            Err("\n\n*[`<gallery>` accepts only one of `path`, `id`]*\n\n".to_owned())
        }
        (None, None) => Err("\n\n*[`<gallery>` requires `path` or `id`]*\n\n".to_owned()),
    }
}

#[derive(Debug)]
pub(super) enum PageLookup {
    Path(String),
    Id(i32),
}

pub(super) fn parse_page_lookup(d: &Directive) -> Result<PageLookup, String> {
    let path = d.arg("path").filter(|s| !s.is_empty());
    let id = d.arg("id").filter(|s| !s.is_empty());

    match (path, id) {
        (Some(p), None) => Ok(PageLookup::Path(p.to_owned())),
        (None, Some(i)) => {
            let n: i32 = i.parse().map_err(|_| {
                "\n\n*[`<page>` got invalid `id` (expected integer)]*\n\n".to_owned()
            })?;
            Ok(PageLookup::Id(n))
        }
        (Some(_), Some(_)) => {
            Err("\n\n*[`<page>` accepts only one of `path`, `id`]*\n\n".to_owned())
        }
        (None, None) => Err("\n\n*[`<page>` requires `path` or `id`]*\n\n".to_owned()),
    }
}

pub(super) async fn fetch_page(
    db: &DatabaseConnection,
    lookup: &PageLookup,
) -> Option<page_entity::Model> {
    match lookup {
        PageLookup::Id(id) => page_entity::Entity::find_by_id(*id)
            .one(db)
            .await
            .ok()
            .flatten(),
        PageLookup::Path(p) => page_entity::Entity::find()
            .filter(page_entity::Column::Path.eq(crate::path_util::normalize(p)))
            .one(db)
            .await
            .ok()
            .flatten(),
    }
}

pub(super) async fn fetch_gallery(
    db: &DatabaseConnection,
    lookup: &GalleryLookup,
) -> Option<gallery_entity::Model> {
    match lookup {
        GalleryLookup::Id(id) => gallery_entity::Entity::find_by_id(*id)
            .one(db)
            .await
            .ok()
            .flatten(),
        GalleryLookup::Path(p) => gallery_entity::Entity::find()
            .filter(gallery_entity::Column::Path.eq(crate::path_util::normalize(p)))
            .one(db)
            .await
            .ok()
            .flatten(),
    }
}
