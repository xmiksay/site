pub mod api;
pub mod broadcast;
pub mod mcp;
pub mod oauth;
pub mod public;
pub mod revision;
pub mod ws;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entity::menu;

#[derive(serde::Serialize, Clone)]
pub struct MenuItem {
    pub path: String,
    pub label: String,
}

#[derive(serde::Serialize, Clone)]
pub struct MenuNode {
    pub path: String,
    pub label: String,
    pub children: Vec<MenuNode>,
}

#[derive(serde::Serialize)]
pub struct Menu {
    pub list: Vec<MenuItem>,
    pub tree: Vec<MenuNode>,
}

pub async fn build_menu(db: &DatabaseConnection, logged_in: bool) -> Menu {
    let mut query = menu::Entity::find().order_by_asc(menu::Column::OrderIndex);
    if !logged_in {
        query = query.filter(menu::Column::Private.eq(false));
    }
    let list: Vec<MenuItem> = query
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|m| !m.path.is_empty())
        .map(|m| MenuItem {
            label: m.title,
            path: format!("/{}", m.path),
        })
        .collect();

    let tree = build_tree(&list);
    Menu { list, tree }
}

/// True when `parent` is a strict segment-prefix of `child`. Both strings are
/// the URL form (with leading slash); `/` is the root and prefixes any path.
fn is_segment_prefix(parent: &str, child: &str) -> bool {
    if parent == child {
        return false;
    }
    if parent == "/" {
        return child.starts_with('/');
    }
    child.starts_with(parent) && child[parent.len()..].starts_with('/')
}

fn build_tree(items: &[MenuItem]) -> Vec<MenuNode> {
    // For each item, find the deepest other item whose path is a segment prefix.
    // Items with no such ancestor become tree roots; their relative order is
    // preserved from the input (which is already ordered by order_index).
    let n = items.len();
    let mut parent_of: Vec<Option<usize>> = vec![None; n];
    for (i, child) in items.iter().enumerate() {
        let mut best: Option<(usize, usize)> = None;
        for (j, candidate) in items.iter().enumerate() {
            if i == j {
                continue;
            }
            if is_segment_prefix(&candidate.path, &child.path) {
                let len = candidate.path.len();
                if best.is_none_or(|(_, blen)| len > blen) {
                    best = Some((j, len));
                }
            }
        }
        parent_of[i] = best.map(|(j, _)| j);
    }

    // Group child indices under each parent (or under `roots` when no parent).
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut root_indices: Vec<usize> = Vec::new();
    for (i, parent) in parent_of.iter().enumerate() {
        match parent {
            Some(p) => children_of[*p].push(i),
            None => root_indices.push(i),
        }
    }

    fn build(idx: usize, items: &[MenuItem], children_of: &[Vec<usize>]) -> MenuNode {
        MenuNode {
            path: items[idx].path.clone(),
            label: items[idx].label.clone(),
            children: children_of[idx]
                .iter()
                .map(|c| build(*c, items, children_of))
                .collect(),
        }
    }

    root_indices
        .into_iter()
        .map(|i| build(i, items, &children_of))
        .collect()
}
