//! The API end to end over real HTTP on a loopback port.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use todai_core::api::{BoardInfo, Events, board_id};
use todai_core::store::{Actor, Kind, Log};

use super::*;

struct Fixture {
    server: Handle,
    log: Log,
    dir: tempfile::TempDir,
    agent: ureq::Agent,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path().join("b.jsonl"));
        log.append(Actor::Agent, |_| {
            Ok(Kind::DecisionAdd { id: "d1".into(), title: "Ship?".into(), context: None, options: vec![] })
        })
        .unwrap();
        let cfg = Config { listen: "127.0.0.1:0".into(), dir: dir.path().join("server"), boards: vec![log.path().into()] };
        let server = Server::bind(&cfg).unwrap().spawn();
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Fixture { server, log, dir, agent }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.server.addr)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.server.token)
    }

    fn get(&self, path: &str) -> (u16, Value) {
        let mut r = self.agent.get(self.url(path)).header("Authorization", self.auth()).call().unwrap();
        (r.status().as_u16(), r.body_mut().read_json().unwrap_or(Value::Null))
    }

    fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let mut r = self.agent.post(self.url(path)).header("Authorization", self.auth()).send_json(body).unwrap();
        (r.status().as_u16(), r.body_mut().read_json().unwrap_or(Value::Null))
    }

    fn id(&self) -> String {
        board_id(self.log.path())
    }
}

#[test]
fn token_is_required_except_for_health() {
    let f = Fixture::new();
    let r = f.agent.get(f.url("/api/health")).call().unwrap();
    assert_eq!(r.status(), 200);
    let r = f.agent.get(f.url("/api/boards")).call().unwrap();
    assert_eq!(r.status(), 401);
    let r = f.agent.get(f.url("/api/boards")).header("Authorization", "Bearer nope").call().unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(f.get("/api/boards").0, 200);
}

#[test]
fn token_and_registry_persist() {
    let f = Fixture::new();
    let dir = f.dir.path().join("server");
    assert_eq!(registry::load_token(&dir).unwrap(), f.server.token);
    let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(dir.join("token")).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(Registry::load(&dir).unwrap().list(), vec![BoardInfo::new(f.log.path().into())]);
    assert_eq!(local(&dir).unwrap(), (f.server.addr, f.server.token.clone()));
}

#[test]
fn lists_and_reads_boards() {
    let f = Fixture::new();
    let (status, boards) = f.get("/api/boards");
    assert_eq!(status, 200);
    let boards: Vec<BoardInfo> = serde_json::from_value(boards).unwrap();
    assert_eq!(boards, vec![BoardInfo::new(f.log.path().into())]);

    let (status, board) = f.get(&format!("/api/boards/{}", f.id()));
    assert_eq!(status, 200);
    assert_eq!(board["state"]["decisions"][0]["title"], "Ship?");
    assert_eq!(board["board"]["name"], "b");

    assert_eq!(f.get("/api/boards/0000000000000000").0, 404);
    assert_eq!(f.get("/api/nope").0, 404);
}

#[test]
fn events_after_seq() {
    let f = Fixture::new();
    let (_, all) = f.get(&format!("/api/boards/{}/events", f.id()));
    let all: Events = serde_json::from_value(all).unwrap();
    assert_eq!((all.events.len(), all.last_seq, all.reset), (1, 1, false));

    let (_, none) = f.get(&format!("/api/boards/{}/events?after=1", f.id()));
    assert_eq!(serde_json::from_value::<Events>(none).unwrap().events, vec![]);

    // Past the end: the file was replaced, so the client gets everything again.
    let (_, reset) = f.get(&format!("/api/boards/{}/events?after=9", f.id()));
    let reset: Events = serde_json::from_value(reset).unwrap();
    assert!(reset.reset);
    assert_eq!(reset.events.len(), 1);

    assert_eq!(f.get(&format!("/api/boards/{}/events?after=x", f.id())).0, 400);
}

#[test]
fn long_poll_wakes_on_append() {
    let f = Fixture::new();
    let log = f.log.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        log.append(Actor::Agent, |s| Ok(Kind::TodoAdd { id: s.next_todo_id(), title: "Push".into(), context: None }))
            .unwrap();
    });
    let start = Instant::now();
    let (_, ev) = f.get(&format!("/api/boards/{}/events?after=1&wait=10", f.id()));
    let ev: Events = serde_json::from_value(ev).unwrap();
    assert!(start.elapsed() < Duration::from_secs(5), "returned on the append, not the timeout");
    assert_eq!(ev.last_seq, 2);
    assert!(matches!(&ev.events[0].kind, Kind::TodoAdd { title, .. } if title == "Push"));
    writer.join().unwrap();

    let start = Instant::now();
    let (_, ev) = f.get(&format!("/api/boards/{}/events?after=2&wait=1", f.id()));
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert_eq!(serde_json::from_value::<Events>(ev).unwrap().events, vec![]);
}

#[test]
fn operator_actions_are_appended_and_validated() {
    let f = Fixture::new();
    let actions = format!("/api/boards/{}/actions", f.id());

    let (status, ev) = f.post(&actions, json!({"type": "decision.resolve", "id": "d1", "text": "yes"}));
    assert_eq!(status, 200, "{ev}");
    assert_eq!((ev["seq"].as_u64(), ev["by"].as_str()), (Some(2), Some("operator")));
    let (_, state) = f.log.load_state().unwrap();
    assert!(matches!(&state.decisions[0].status, todai_core::store::DecisionStatus::Resolved { text, .. } if text == "yes"));

    // The server assigns elaboration ids, whatever the client sent.
    let ask = json!({"type": "elab.request", "rid": "", "target": "d1", "selection": "Ship"});
    let (status, ev) = f.post(&actions, ask);
    assert_eq!((status, ev["rid"].as_str()), (200, Some("r1")));

    let (status, err) = f.post(&actions, json!({"type": "decision.resolve", "id": "d9", "text": "x"}));
    assert_eq!(status, 422);
    assert!(err["error"].as_str().unwrap().contains("d9"), "{err}");

    let agent_kind = json!({"type": "todo.add", "id": "t1", "title": "sneaky"});
    assert_eq!(f.post(&actions, agent_kind).0, 403);
    assert_eq!(f.post(&actions, json!({"type": "nope"})).0, 400);
    assert_eq!(f.log.load().unwrap().len(), 3, "refused actions are never written");
}

#[test]
fn register_and_forget() {
    let f = Fixture::new();
    let other = f.dir.path().join("other.jsonl");
    let (status, _) = f.post("/api/boards", json!({"path": other}));
    assert_eq!(status, 422, "the file must exist");
    std::fs::write(&other, "").unwrap();
    assert_eq!(f.post("/api/boards", json!({"path": "rel.jsonl"})).0, 422);
    assert_eq!(f.post("/api/boards", json!({"path": f.dir.path().join("server/token")})).0, 422);

    let (status, info) = f.post("/api/boards", json!({"path": other}));
    assert_eq!(status, 201);
    assert_eq!(info["id"], board_id(&other));
    assert_eq!(f.get("/api/boards").1.as_array().unwrap().len(), 2);

    let del = |id: &str| {
        let r = f.agent.delete(f.url(&format!("/api/boards/{id}"))).header("Authorization", f.auth()).call();
        r.unwrap().status().as_u16()
    };
    assert_eq!(del(&board_id(&other)), 204);
    assert_eq!(del(&board_id(&other)), 404);
    assert_eq!(f.get("/api/boards").1.as_array().unwrap().len(), 1);
}

#[test]
fn forget_from_the_cli_reaches_a_running_server() {
    let f = Fixture::new();
    // What `todai-server forget` does: a second Registry on the same dir.
    assert!(Registry::load(&f.dir.path().join("server")).unwrap().remove(&f.id()).unwrap());
    assert_eq!(f.get("/api/boards").1, json!([]));
    assert_eq!(f.get(&format!("/api/boards/{}", f.id())).0, 404);
}
