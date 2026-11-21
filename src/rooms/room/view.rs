//! How everyone in a room sees the map, as an admin sets it.

use std::sync::Arc;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::Room;
use super::state::{Leader, on_map};
use crate::cameras::{Change, Kept, Row, Shot, Spot};
use crate::matches::hub::announce;
use crate::sync::lock;

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

/// Where an admin puts the relay's camera (neither: nowhere), and the path it sets off on.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SpotPost {
    #[serde(default)]
    pub(crate) pos: Option<[i32; 3]>,
    #[serde(default)]
    pub(crate) aim: Option<[i32; 3]>,
    #[serde(default)]
    pub(crate) path: Option<Vec<Row>>,
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
            bail!("cannot set a view off the map");
        }
        if post.page.len() > MAX_PAGE {
            bail!("cannot take a page name over {MAX_PAGE} bytes");
        }
        let by = lock(&self.people).id(guest);
        let mut leader = lock(&self.leader);
        let kind = view.kind();
        if !post.take && leader.as_ref().is_none_or(|l| l.kind != kind) {
            return Ok(());
        }
        let was = leader.replace(Leader { by, page: post.page.clone(), kind: kind.to_string() });
        if kind == "follow"
            && let Some(was) = was.filter(|l| l.kind != "follow")
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
        // A player chosen takes the relay's camera off its spot.
        let off_spot = post.take && camera.is_some() && self.leave_spot();
        if std::mem::replace(&mut *lock(&self.camera), camera) != camera || off_spot {
            self.steer_relay(&lock(&self.state));
        }
        // Chosen, or moved by hand (a page does not send what it does by itself, such as a follow
        // dropped at a map change): someone is at the controls.
        self.stop_following_live();
        Ok(())
    }

    /// The room went to another map: a player followed is not on it. The view goes back to the
    /// one before the follow (free if none), as the leader's page would on stopping; the relay's
    /// camera with it. Nobody touched the room.
    pub(super) fn end_follow(&self) {
        let mut leader = lock(&self.leader);
        let Some(Leader { by, page, .. }) = leader.clone().filter(|l| l.kind == "follow") else { return };
        let free = || {
            let line = serde_json::json!({ "type": "view", "mode": "free", "by": by, "page": page });
            (Leader { by, page: page.clone(), kind: "free".to_string() }, line.to_string().into())
        };
        let (was, line): (Leader, Arc<str>) = lock(&self.before_follow).take().unwrap_or_else(free);
        *leader = Some(was);
        *lock(&self.view) = Some(line.clone());
        announce(&self.hub, "view", &line);
        drop(leader);
        *lock(&self.camera) = None;
    }

    /// An admin puts the relay's camera at a spot of the map, or (none) back with the director.
    /// A path given sets it off anew, the same again too.
    pub(crate) fn set_spot(&self, post: &SpotPost) -> Result<()> {
        let now = match (post.pos, post.aim) {
            (Some(pos), Some(aim)) => Some(Shot::new(Spot { pos, aim }, post.path.as_deref())?),
            (None, None) if post.path.is_none() => None,
            _ => bail!("Invalid camera."),
        };
        let at = now.as_ref().map(|s| s.spot);
        let mut shot = lock(&self.spot);
        // Staying where it stays is no change.
        let stays = |s: &Shot| (s.spot, s.path.steps.len() == 1);
        if post.path.is_none() && shot.as_ref().map(stays) == at.map(|s| (s, true)) {
            return Ok(());
        }
        let moving = |s: &Shot| s.path.steps.len() > 1;
        let told = (at, now.as_ref().is_some_and(moving));
        let was = std::mem::replace(&mut *shot, now);
        drop(shot);
        if was.map(|s| (Some(s.spot), moving(&s))) != Some(told) {
            self.tell_spot(told.0, told.1);
        }
        self.steer_relay(&lock(&self.state));
        Ok(())
    }

    /// An admin's change to the room's own cameras on its map.
    pub(crate) fn change_cameras(&self, change: &Change) -> Result<()> {
        let Some(map) = lock(&self.hub).map.clone() else { bail!("No map yet.") };
        let mut own = lock(&self.cameras);
        own.change(&map, change)?;
        self.tell_cameras(&own);
        Ok(())
    }

    /// The room's own cameras, as saved.
    pub(super) fn keep_cameras(&self, saved: Kept) {
        let mut own = lock(&self.cameras);
        *own = saved;
        self.tell_cameras(&own);
    }

    fn tell_cameras(&self, own: &Kept) {
        let line = serde_json::json!({ "type": "cameras", "on": own.by_map() });
        announce(&self.hub, "cameras", &Arc::from(line.to_string()));
    }

    /// The relay's camera leaves its spot, as on another map. Whether it was at one.
    pub(super) fn leave_spot(&self) -> bool {
        let was = lock(&self.spot).take().is_some();
        if was {
            self.tell_spot(None, false);
        }
        was
    }

    /// Tells the pages where the relay's camera is, and whether it set off from there.
    fn tell_spot(&self, spot: Option<Spot>, moving: bool) {
        let line = match spot {
            Some(s) if moving => serde_json::json!({ "type": "camera", "pos": s.pos, "aim": s.aim, "moving": true }),
            Some(s) => serde_json::json!({ "type": "camera", "pos": s.pos, "aim": s.aim }),
            None => serde_json::json!({ "type": "camera" }),
        };
        announce(&self.hub, "camera", &Arc::from(line.to_string()));
    }

    /// The relay's camera: at the spot, else on the player followed.
    pub(super) fn relay_camera(&self) -> Option<stvtf::relay::Camera> {
        let follow = lock(&self.camera).map(|player| stvtf::relay::Camera::Follow { player });
        lock(&self.spot).as_ref().map(Shot::for_tf2).or(follow)
    }
}
