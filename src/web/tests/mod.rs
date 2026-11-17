mod comms;
mod files;
mod harness;
mod match_pages;
mod room_pages;
mod streams;
mod uploads;
mod voice;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use flate2::read::GzDecoder;
use stvtf::Line;
use stvtf::output::{self, PlayerId};

use self::harness::{
    Served, app_files, asks, cookie_of, demo_app, exchange, get, head_and_body, invite_of, invited, open_room, page_of, post, post_watch,
    request_to, sessions_of, signed_in, site_app, status, voice_app,
};
use super::assets::app_version;
use super::client::counted;
use super::*;
use crate::app::{App, Matches};
use crate::matches::hub::{SharedHub, publish};
use crate::matches::sessions;
use crate::matches::sessions::Sessions;
use crate::testing::{TempDir, TestApp, app, server, web_server};
use crate::web::answer::unix_now;
use crate::web::client::{client_key, client_of, is_cloudflare, on_our_network};
use crate::web::content::{CSS, HTML, JS, SVG, WEBP};
use crate::web::limits::MAX_REQUEST;
use crate::web::matches::start_view;
