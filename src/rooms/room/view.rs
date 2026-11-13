//! How everyone in a room sees the map, as an admin sets it.

use super::{Leader, Room, on_map};
use crate::lock;
use crate::matches::hub::announce;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How everyone in the room sees the map, as an admin sets it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub(crate) enum View {
    /// Each their own camera.
    Free,
    /// Everyone on auto zoom.
    Auto,
    /// Everyone following one player, seeing as much of the map round them as the admin: `w` by
    /// `h`.
    Follow {
        player: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        w: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        h: Option<f64>,
    },
    /// Everyone seeing what the admin sees: the map area `w` by `h` round `cx`, `cy`, turned by
    /// `rot`.
    Admin { cx: f64, cy: f64, w: f64, h: f64, rot: f64 },
}

impl View {
    /// What kind of view it is.
    fn kind(&self) -> &'static str {
        match self {
            View::Free => "free",
            View::Auto => "auto",
            View::Follow { .. } => "follow",
            View::Admin { .. } => "admin",
        }
    }
}

/// A view as an admin's page sends it: `take` when they chose it, which sets the kind of view;
/// otherwise it is their camera moving, which counts only while the view is of that kind (a move
/// sent before someone chose another no longer stands). Either way that page leads from then on:
/// the latest to choose or move it. `page` tells an admin's pages apart (two tabs, say).
#[derive(Debug, Deserialize)]
pub(crate) struct ViewPost {
    #[serde(flatten)]
    pub(crate) view: View,
    #[serde(default)]
    pub(crate) take: bool,
    #[serde(default)]
    pub(crate) page: String,
}

/// The longest page name.
pub(super) const MAX_PAGE: usize = 32;

impl Room {
    /// Admin `guest` sets how everyone sees the map.
    pub(crate) fn set_view(&self, post: &ViewPost, guest: &str) -> Result<()> {
        let view = &post.view;
        let area = |vs: &[f64], w: f64, h: f64| vs.iter().all(|&v| on_map(v)) && w > 0.0 && h > 0.0;
        let fits = match *view {
            View::Admin { cx, cy, w, h, rot } => area(&[cx, cy, w, h, rot], w, h),
            View::Follow { w: Some(w), h: Some(h), .. } => area(&[w, h], w, h),
            View::Follow { w: None, h: None, .. } | View::Free | View::Auto => true,
            View::Follow { .. } => false,
        };
        if !fits {
            bail!("a view must be on a map");
        }
        if post.page.len() > MAX_PAGE {
            bail!("a page name is at most {MAX_PAGE} bytes");
        }
        let by = lock(&self.people).id(guest);
        let mut leader = lock(&self.leader);
        let kind = view.kind();
        if !post.take && leader.as_ref().is_none_or(|(_, _, now)| now != kind) {
            return Ok(());
        }
        let was = leader.replace((by, post.page.clone(), kind.to_string()));
        if kind == "follow"
            && let Some(was) = was.filter(|(_, _, k)| k != "follow")
        {
            let line = lock(&self.view).clone();
            *lock(&self.before_follow) = line.map(|line| (was, line));
        }
        let mut line = serde_json::to_value(view)?;
        line["type"] = "view".into();
        line["by"] = by.into();
        line["page"] = post.page.clone().into();
        let line: Arc<str> = line.to_string().into();
        *lock(&self.view) = Some(line.clone());
        announce(&self.hub, "view", &line);
        drop(leader);
        let camera = match *view {
            View::Follow { player, .. } => Some(player),
            _ => None,
        };
        if std::mem::replace(&mut *lock(&self.camera), camera) != camera {
            self.follow(&lock(&self.state));
        }
        // Chosen, or moved by hand (a page does not send what it does by itself, such as a follow
        // dropped at a map change): someone is at the controls.
        self.touched();
        Ok(())
    }

    /// The room went to another map: a player followed is not on it. The view goes back to the
    /// one before the follow (free if none), as the leader's page would on stopping; the relay's
    /// camera with it. Nobody touched the room.
    pub(super) fn end_follow(&self) {
        let mut leader = lock(&self.leader);
        let Some((by, page, _)) = leader.clone().filter(|(_, _, kind)| kind == "follow") else { return };
        let free = || {
            let line = serde_json::json!({ "type": "view", "mode": "free", "by": by, "page": page });
            ((by, page.clone(), "free".to_string()), line.to_string().into())
        };
        let (was, line): (Leader, Arc<str>) = lock(&self.before_follow).take().unwrap_or_else(free);
        *leader = Some(was);
        *lock(&self.view) = Some(line.clone());
        announce(&self.hub, "view", &line);
        drop(leader);
        *lock(&self.camera) = None;
    }
}
