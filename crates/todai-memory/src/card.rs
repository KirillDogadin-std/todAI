//! Cards: one remembered decision each, with the keys it is found by.

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const MAX_GIST: usize = 120;
pub const MAX_KEYS: usize = 5;
pub const MAX_KEY_CHARS: usize = 40;
pub const MAX_KEY_WORDS: usize = 5;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Must always hold in this project.
    Axiom,
    /// Holds in the situation it was decided in.
    Context,
    /// Settled once; unlikely to matter again.
    OneOff,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Axiom => "axiom",
            Class::Context => "context",
            Class::OneOff => "one_off",
        }
    }

    pub fn parse(s: &str) -> Result<Class> {
        match s.replace('-', "_").as_str() {
            "axiom" => Ok(Class::Axiom),
            "context" => Ok(Class::Context),
            "one_off" => Ok(Class::OneOff),
            _ => bail!("unknown class {s:?}: axiom, context or one_off"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    /// An axiom an agent proposed; the operator hasn't confirmed it.
    Proposed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Proposed => "proposed",
        }
    }
}

/// What a writer supplies; [`CardDraft::validate`] normalises it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct CardDraft {
    pub class: Option<Class>,
    pub gist: String,
    /// The situation it was decided in.
    pub context: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub applies: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub not_when: String,
    pub why: String,
    /// Alternatives and why not.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rejected: String,
    pub keys: Vec<String>,
}

impl CardDraft {
    /// Trims fields, normalises and dedupes keys, and checks the rules a
    /// card must meet. Every problem is reported at once.
    pub fn validate(mut self) -> Result<CardDraft> {
        for f in [&mut self.gist, &mut self.context, &mut self.applies, &mut self.not_when, &mut self.why, &mut self.rejected] {
            *f = f.trim().to_owned();
        }
        let mut problems = Vec::new();
        let gist_len = self.gist.chars().count();
        if gist_len == 0 {
            problems.push("gist is empty".to_owned());
        } else if gist_len > MAX_GIST {
            problems.push(format!("gist is {gist_len} characters; at most {MAX_GIST}"));
        }
        if self.gist.contains('\n') {
            problems.push("gist must be one line".to_owned());
        }
        if self.context.is_empty() {
            problems.push("context is empty: say what situation this was decided in".to_owned());
        }
        if self.why.is_empty() {
            problems.push("why is empty".to_owned());
        }
        let mut keys: Vec<String> = Vec::new();
        for raw in &self.keys {
            let k = normalize_key(raw);
            if k.is_empty() {
                continue;
            }
            let words = k.split(' ').count();
            if words > MAX_KEY_WORDS || k.chars().count() > MAX_KEY_CHARS {
                problems.push(format!("key {k:?} is too long: at most {MAX_KEY_WORDS} words and {MAX_KEY_CHARS} characters"));
            }
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        if keys.is_empty() {
            problems.push("no keys: give 1-5 short phrases it should be found by".to_owned());
        } else if keys.len() > MAX_KEYS {
            problems.push(format!("{} keys; at most {MAX_KEYS}", keys.len()));
        }
        self.keys = keys;
        if !problems.is_empty() {
            bail!("not a valid card:\n- {}", problems.join("\n- "));
        }
        Ok(self)
    }
}

/// Lowercase, single spaces, no punctuation at either end:
/// `"  GPU  Backend. "` → `"gpu backend"`. No stemming.
pub fn normalize_key(raw: &str) -> String {
    let words: Vec<String> = raw.split_whitespace().map(str::to_lowercase).collect();
    let joined = words.join(" ");
    joined.trim_matches(|c: char| !c.is_alphanumeric()).to_owned()
}

/// A stored card: the draft plus what the store assigned.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Card {
    pub id: String,
    pub class: Class,
    pub status: Status,
    pub gist: String,
    pub context: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub applies: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub not_when: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rejected: String,
    pub keys: Vec<String>,
    /// Milliseconds since the Unix epoch.
    pub created: i64,
    pub created_by: crate::Actor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::Source>,
}

/// Payload of a `created` event: everything needed to rebuild the card.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Created {
    pub class: Class,
    pub status: Status,
    #[serde(flatten)]
    pub draft: CardDraft,
}

// ---- storage (called inside `Memory`'s transactions) ----

/// `m` + 8 hex digits of a hash of the card and the time; longer if those
/// are taken (rare: a clash needs the same 32-bit hash).
pub(crate) fn new_id(conn: &Connection, draft: &CardDraft) -> Result<String> {
    let seed = format!("{}\n{}\n{}", crate::now_ms(), draft.gist, draft.context);
    let hex = format!("{:016x}", crate::fnv1a(seed.as_bytes()));
    for len in [8, 12, 16] {
        let id = format!("m{}", &hex[..len]);
        let taken: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM card WHERE id = ?1)", [&id], |r| r.get(0))?;
        if !taken {
            return Ok(id);
        }
    }
    bail!("no free card id (hash {hex} taken at every length)")
}

/// The derived rows that make a new card findable.
pub(crate) fn index(conn: &Connection, id: &str, c: &Created, seq: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO card (id, class, status, gist, created) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, c.class.as_str(), c.status.as_str(), c.draft.gist, seq],
    )?;
    for k in &c.draft.keys {
        conn.execute("INSERT OR IGNORE INTO key (phrase) VALUES (?1)", [k])?;
        conn.execute("INSERT INTO tagged (key, card) SELECT id, ?2 FROM key WHERE phrase = ?1", params![k, id])?;
    }
    Ok(())
}

pub(crate) fn from_created(id: String, c: Created, e: &crate::Event) -> Card {
    let d = c.draft;
    Card {
        id,
        class: c.class,
        status: c.status,
        gist: d.gist,
        context: d.context,
        applies: d.applies,
        not_when: d.not_when,
        why: d.why,
        rejected: d.rejected,
        keys: d.keys,
        created: e.ts,
        created_by: e.actor,
        source: e.source.clone(),
    }
}

/// Reads a card from its `created` event: the event is the only copy of
/// the reasoning; the `card` row only points at it.
fn load(conn: &Connection, id: &str) -> Result<Card> {
    let (status, seq): (String, i64) =
        conn.query_row("SELECT status, created FROM card WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let (ts, actor, payload, source): (i64, String, String, Option<String>) = conn.query_row(
        "SELECT ts, actor, payload, source FROM event WHERE seq = ?1",
        [seq],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let mut created: Created = serde_json::from_str(&payload)?;
    created.status = match status.as_str() {
        "proposed" => Status::Proposed,
        _ => Status::Active,
    };
    let event = crate::Event {
        seq,
        ts,
        actor: crate::Actor::parse(&actor)?,
        card: Some(id.to_owned()),
        kind: "created".into(),
        payload: serde_json::Value::Null,
        source: source.map(|s| serde_json::from_str(&s)).transpose()?,
    };
    Ok(from_created(id.to_owned(), created, &event))
}

pub(crate) fn get(conn: &Connection, id: &str) -> Result<Option<Card>> {
    let id = id.trim();
    let exact: Option<String> = conn.query_row("SELECT id FROM card WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    if let Some(id) = exact {
        return Ok(Some(load(conn, &id)?));
    }
    // A unique prefix, like a short git hash; at least `m` + 3 digits.
    if id.len() < 4 || !id.starts_with('m') || !id[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let mut stmt = conn.prepare("SELECT id FROM card WHERE id >= ?1 AND id < ?1 || 'g' ORDER BY id LIMIT 6")?;
    let ids: Vec<String> = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    match ids.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(load(conn, one)?)),
        many => bail!("{id} is ambiguous: {}", many.join(", ")),
    }
}

pub(crate) fn find(conn: &Connection, keys: &[String]) -> Result<Vec<Card>> {
    let keys: Vec<String> = keys.iter().map(|k| normalize_key(k)).filter(|k| !k.is_empty()).collect();
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; keys.len()].join(", ");
    let sql = format!(
        "SELECT c.id FROM card c JOIN tagged t ON t.card = c.id JOIN key k ON k.id = t.key
         WHERE k.phrase IN ({marks})
         GROUP BY c.id
         ORDER BY count(*) DESC,
                  CASE c.class WHEN 'axiom' THEN 0 WHEN 'context' THEN 1 ELSE 2 END,
                  c.created DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids: Vec<String> =
        stmt.query_map(rusqlite::params_from_iter(&keys), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    ids.iter().map(|id| load(conn, id)).collect()
}

pub(crate) fn keys(conn: &Connection) -> Result<Vec<(String, usize)>> {
    let mut stmt = conn.prepare(
        "SELECT k.phrase, count(t.card) FROM key k LEFT JOIN tagged t ON t.key = k.id
         GROUP BY k.id ORDER BY count(t.card) DESC, k.phrase",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
