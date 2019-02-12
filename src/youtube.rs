use log::*;
use once_cell::sync::Lazy;
use once_cell::sync_lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::database;
use crate::error::{Error, Result};
use crate::server;
use crate::FromRow;

static PATTERN: Lazy<Regex> = sync_lazy! {
    Regex::new(
        r#"(:?^(:?http?.*?youtu(:?\.be|be.com))(:?/|.*?v=))(?P<id>[A-Za-z0-9_-]{11})"#,
    ).expect("valid regex")
};

static API_KEY: Lazy<String> = sync_lazy! {
    const YOUTUBE_API_KEY: &str = "SHAKEN_YOUTUBE_API_KEY";
    std::env::var(YOUTUBE_API_KEY).map_err(|_| {
        error!("environment var `{}` must be set",YOUTUBE_API_KEY );
        std::process::exit(1);
    }).unwrap()
};

#[derive(Serialize)]
pub struct Song {
    pub id: i64,
    pub vid: String,
    pub timestamp: i64,
    pub duration: i64,
    pub title: String,
}

impl FromRow for Song {
    fn from_row(row: &rusqlite::Row<'_, '_>) -> Self {
        Self {
            id: row.get(0),
            vid: row.get(1),
            timestamp: row.get(2),
            duration: row.get(3),
            title: row.get(4),
        }
    }

    fn timestamp(&self) -> i64 {
        self.timestamp
    }
}

#[derive(Default)]
pub struct Youtube;

impl crate::Storage<Song> for Youtube {
    fn insert(&self, item: &server::Item) -> Result<()> {
        let url = match &item.kind {
            server::ItemKind::Youtube(url) => url,
            _ => unreachable!("expected a youtube item"),
        };

        let id = PATTERN
            .captures(&url)
            .and_then(|s| s.name("id"))
            .map(|s| s.as_str())
            .ok_or_else(|| Error::InvalidYoutubeUrl(url.to_string()))?;

        let info = YoutubeItem::fetch(id)?;

        database::get_connection()
            .execute_named(
                include_str!("../sql/youtube/add_video.sql"),
                &[
                    (":vid", &id),
                    (":ts", &item.ts),
                    (":duration", &info.duration),
                    (":title", &info.title),
                ],
            )
            .map_err(Error::Sql)
            .map(|_| ())
    }

    fn current(&self) -> Result<Song> {
        database::get_connection()
            .query_row(
                include_str!("../sql/youtube/get_current.sql"),
                rusqlite::NO_PARAMS,
                Song::from_row,
            )
            .map_err(Error::Sql)
    }

    fn previous(&self) -> Result<Song> {
        database::get_connection()
            .query_row(
                include_str!("../sql/youtube/get_previous.sql"),
                rusqlite::NO_PARAMS,
                Song::from_row,
            )
            .map_err(Error::Sql)
    }

    fn all(&self) -> Result<Vec<Song>> {
        Ok(database::get_connection()
            .prepare(include_str!("../sql/youtube/get_all.sql"))?
            .query_map(rusqlite::NO_PARAMS, Song::from_row)
            .map_err(Error::Sql)?
            .filter_map(|s| s.ok())
            .collect::<Vec<_>>())
    }
}

pub struct YoutubeItem {
    pub title: String,
    pub duration: i64,
}

impl YoutubeItem {
    pub fn fetch(id: &str) -> Result<Self> {
        const BASE: &str = "https://www.googleapis.com/youtube/v3";
        let query = Self::build_query(id)?;

        let mut data = vec![];
        let resp = http_req::request::get(format!("{}/videos/?{}", BASE, query), &mut data)
            .map_err(Error::HttpClient)?;

        if !resp.status_code().is_success() {
            return Err(Error::HttpResponse(
                resp.status_code().into(),
                resp.reason().to_string(),
            ));
        }

        Self::serialize(&data)
    }

    fn serialize(data: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        struct Response<'a> {
            #[serde(borrow)]
            items: Vec<Item<'a>>,
        }
        #[derive(Deserialize)]
        struct Item<'a> {
            #[serde(borrow)]
            snippet: Snippet<'a>,
            #[serde(borrow, rename = "contentDetails")]
            details: ContentDetails<'a>,
        }
        #[derive(Deserialize)]
        struct Snippet<'a> {
            title: &'a str,
        }
        #[derive(Deserialize)]
        struct ContentDetails<'a> {
            duration: &'a str,
        }

        let data = serde_json::from_slice::<Response>(&data).map_err(Error::Serialize)?;
        let item = &data.items.get(0).ok_or_else(|| Error::InvalidYoutubeData)?;
        Ok(Self {
            title: item.snippet.title.to_string(),
            duration: from_iso8601(item.details.duration),
        })
    }

    fn build_query(id: &str) -> Result<String> {
        let map = &[
            ("id", id),
            ("part", "snippet,contentDetails"),
            (
                "fields",
                "items(id, snippet(title), contentDetails(duration))",
            ),
            ("key", API_KEY.as_str()),
        ];

        Ok(map
            .as_ref()
            .iter()
            .map(|(k, v)| format!("{}={}&", encode(k), encode(v)))
            .collect())
    }
}

#[inline]
fn encode(data: &str) -> String {
    data.chars().fold(String::new(), |mut a, ch| {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => a.push(ch),
            ch => a.push_str(&format!("%{:02X}", ch as u32)),
        }
        a
    })
}

#[inline]
fn from_iso8601(period: &str) -> i64 {
    let parse = |s, e| period[s + 1..e].parse::<i64>().unwrap_or(0);
    period
        .chars()
        .enumerate()
        .fold((0, 0), |(a, p), (i, c)| match c {
            c if c.is_numeric() => (a, p),
            'H' => (a + parse(p, i) * 60 * 60, i),
            'M' => (a + parse(p, i) * 60, i),
            'S' => (a + parse(p, i), i),
            'P' | 'T' | _ => (a, i),
        })
        .0 as i64
}
