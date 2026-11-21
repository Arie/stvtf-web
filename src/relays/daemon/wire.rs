//! What stv.tf and the relay process say to each other, a JSON line each.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use stvtf::relay::{Beam, Camera, Light, Names, Note, Path as CameraPath, Step, VoiceIn, Want};

use crate::relays::pool::Was;

/// A [`Want`] on the wire.
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct WantLine {
    pub(super) demo: PathBuf,
    pub(super) tick: u32,
    pub(super) age_ms: u64,
    pub(super) paused: bool,
    pub(super) speed: f64,
    pub(super) from_start: bool,
    /// The player (user id) the camera follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) follow: Option<u16>,
    /// The path it is on instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) path: Option<PathLine>,
    /// The spot it is at instead, from a stv.tf of before paths: where, pitch, yaw.
    #[serde(default, skip_serializing)]
    pub(super) fixed: Option<([i32; 3], i16, i16)>,
}

/// A camera's path on the wire: each moment the seconds in, where it is, what it looks at.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct PathLine {
    pub(super) id: u64,
    pub(super) age_ms: u64,
    pub(super) steps: Vec<[f32; 7]>,
}

impl PathLine {
    pub(super) fn of(p: &CameraPath) -> PathLine {
        let steps = p.steps.iter().map(|s| [s.secs, s.pos[0], s.pos[1], s.pos[2], s.aim[0], s.aim[1], s.aim[2]]);
        PathLine { id: p.id, age_ms: p.began.elapsed().as_millis() as u64, steps: steps.collect() }
    }

    pub(super) fn path(self) -> CameraPath {
        let began = Instant::now().checked_sub(Duration::from_millis(self.age_ms)).unwrap_or_else(Instant::now);
        let steps = self.steps.iter().map(|r| Step { secs: r[0], pos: [r[1], r[2], r[3]], aim: [r[4], r[5], r[6]] });
        CameraPath { id: self.id, began, steps: steps.collect() }
    }
}

/// A spot from a stv.tf of before paths, as a path: each spot its own.
pub(super) fn still(pos: [i32; 3], pitch: i16, yaw: i16) -> CameraPath {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (pos, pitch, yaw).hash(&mut hash);
    let id = hash.finish();
    let (pitch, yaw) = (f32::from(pitch).to_radians(), f32::from(yaw).to_radians());
    let pos = pos.map(|v| v as f32);
    let aim = [pos[0] + 1000.0 * pitch.cos() * yaw.cos(), pos[1] + 1000.0 * pitch.cos() * yaw.sin(), pos[2] - 1000.0 * pitch.sin()];
    CameraPath { id, began: Instant::now(), steps: vec![Step { secs: 0.0, pos, aim }] }
}

impl WantLine {
    pub(super) fn of(w: &Want) -> WantLine {
        WantLine {
            demo: w.demo.clone(),
            tick: w.tick,
            age_ms: w.at.elapsed().as_millis() as u64,
            paused: w.paused,
            speed: w.speed,
            from_start: w.from_start,
            follow: match w.camera {
                Some(Camera::Follow { player }) => Some(player),
                _ => None,
            },
            path: match &w.camera {
                Some(Camera::Along(path)) => Some(PathLine::of(path)),
                _ => None,
            },
            fixed: None,
        }
    }

    pub(super) fn want(self) -> Want {
        let at = Instant::now().checked_sub(Duration::from_millis(self.age_ms)).unwrap_or_else(Instant::now);
        Want {
            demo: self.demo,
            tick: self.tick,
            at,
            paused: self.paused,
            speed: self.speed,
            from_start: self.from_start,
            camera: self
                .path
                .map(PathLine::path)
                .or(self.fixed.map(|(pos, pitch, yaw)| still(pos, pitch, yaw)))
                .map(|path| Camera::Along(Arc::new(path)))
                .or(self.follow.map(|player| Camera::Follow { player })),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub(super) enum Request {
    Start {
        room: String,
        want: WantLine,
        #[serde(default)]
        was: Option<Was>,
        /// The room knows where it is: `want` is no stand-in.
        #[serde(default = "yes")]
        known: bool,
    },
    Follow {
        room: String,
        want: WantLine,
    },
    Status {
        room: String,
    },
    Keep {
        room: String,
        beams: Vec<BeamLine>,
    },
    Notes {
        room: String,
        notes: Vec<Note>,
    },
    Names {
        room: String,
        names: Names,
    },
    Score {
        room: String,
        score: [i64; 2],
    },
    Voice {
        room: String,
        voice: Vec<VoiceLine>,
    },
    Light {
        room: String,
        light: Option<Light>,
    },
    Stop {
        room: String,
    },
    Clients {},
}

/// A [`VoiceIn`] on the wire: its frames in base64.
#[derive(Deserialize, Serialize)]
pub(super) struct VoiceLine {
    pub(super) user: Option<u16>,
    pub(super) steamid: String,
    pub(super) rate: u16,
    pub(super) frames: Vec<String>,
}

impl VoiceLine {
    pub(super) fn of(v: &VoiceIn) -> Self {
        let b64 = base64::engine::general_purpose::STANDARD;
        VoiceLine { user: v.user, steamid: v.steamid.clone(), rate: v.rate, frames: v.frames.iter().map(|f| b64.encode(f)).collect() }
    }

    /// None when a frame is not base64.
    pub(super) fn voice(&self) -> Option<VoiceIn> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let frames = self.frames.iter().map(|f| b64.decode(f).ok()).collect::<Option<_>>()?;
        Some(VoiceIn { user: self.user, steamid: self.steamid.clone(), rate: self.rate, frames })
    }
}

/// The most voice waiting for the relay process; past it the oldest goes.
pub(super) const MAX_VOICE_WAITING: usize = 256;

/// A [`Beam`] on the wire.
pub(super) type BeamLine = [f32; 12];

pub(super) fn beam_line(b: &Beam) -> BeamLine {
    let [r, g, bl] = b.rgb.map(f32::from);
    [b.from[0], b.from[1], b.from[2], b.to[0], b.to[1], b.to[2], r, g, bl, b.life, b.width, f32::from(b.speed)]
}

pub(super) fn beam(l: &BeamLine) -> Beam {
    let byte = |v: f32| v.clamp(0.0, 255.0) as u8;
    Beam {
        from: [l[0], l[1], l[2]],
        to: [l[3], l[4], l[5]],
        rgb: [byte(l[6]), byte(l[7]), byte(l[8])],
        life: l[9],
        width: l[10],
        speed: byte(l[11]),
    }
}

pub(super) fn yes() -> bool {
    true
}
