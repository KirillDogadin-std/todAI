use anyhow::{Result, bail};
use serde::Serialize;

use super::event::{Actor, Event, Kind};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum DecisionStatus {
    Pending,
    Resolved { text: String, seq: u64 },
    Withdrawn,
}

#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub id: String,
    pub title: String,
    pub context: Option<String>,
    pub options: Vec<String>,
    #[serde(flatten)]
    pub status: DecisionStatus,
}

#[derive(Clone, Debug, Serialize)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub context: Option<String>,
    /// `Some(note)` once done; the inner option is the operator's optional note.
    pub done: Option<Option<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Elab {
    pub rid: String,
    pub target: String,
    pub selection: String,
    pub note: Option<String>,
    pub answer: Option<String>,
}

/// Current view of a log, produced by folding its events in order.
#[derive(Clone, Debug, Default, Serialize)]
pub struct State {
    pub decisions: Vec<Decision>,
    pub todos: Vec<Todo>,
    pub elabs: Vec<Elab>,
    pub last_seq: u64,
    pub acked_seq: u64,
}

impl State {
    pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> Result<State> {
        let mut s = State::default();
        for e in events {
            s.apply(e)?;
        }
        Ok(s)
    }

    pub fn decision(&self, id: &str) -> Option<&Decision> {
        self.decisions.iter().find(|d| d.id == id)
    }

    pub fn todo(&self, id: &str) -> Option<&Todo> {
        self.todos.iter().find(|t| t.id == id)
    }

    pub fn elab(&self, rid: &str) -> Option<&Elab> {
        self.elabs.iter().find(|r| r.rid == rid)
    }

    pub fn next_decision_id(&self) -> String {
        format!("d{}", self.decisions.len() + 1)
    }

    pub fn next_todo_id(&self) -> String {
        format!("t{}", self.todos.len() + 1)
    }

    pub fn next_elab_id(&self) -> String {
        format!("r{}", self.elabs.len() + 1)
    }

    /// Checks that `kind` is a legal next event from `by`, without applying it.
    pub fn validate(&self, by: Actor, kind: &Kind) -> Result<()> {
        if let Some(allowed) = kind.allowed_actor()
            && allowed != by
        {
            bail!("{:?} may not emit this event", by);
        }
        match kind {
            Kind::DecisionAdd { id, .. } if self.decision(id).is_some() => {
                bail!("decision {id} already exists")
            }
            Kind::DecisionResolve { id, .. } => match self.decision(id).map(|d| &d.status) {
                None => bail!("no decision {id}"),
                Some(DecisionStatus::Withdrawn) => bail!("decision {id} was withdrawn"),
                Some(_) => {}
            },
            Kind::DecisionReopen { id } => match self.decision(id).map(|d| &d.status) {
                None => bail!("no decision {id}"),
                Some(DecisionStatus::Resolved { .. }) => {}
                Some(_) => bail!("decision {id} is not resolved"),
            },
            Kind::DecisionWithdraw { id } if self.decision(id).is_none() => {
                bail!("no decision {id}")
            }
            Kind::TodoAdd { id, .. } if self.todo(id).is_some() => {
                bail!("todo {id} already exists")
            }
            Kind::TodoDone { id, .. } | Kind::TodoUndone { id } if self.todo(id).is_none() => {
                bail!("no todo {id}")
            }
            Kind::ElabRequest { rid, target, .. } => {
                if self.elab(rid).is_some() {
                    bail!("elaboration {rid} already exists");
                }
                if self.decision(target).is_none() && self.todo(target).is_none() {
                    bail!("no item {target}");
                }
            }
            Kind::ElabAdd { rid, .. } if self.elab(rid).is_none() => {
                bail!("no elaboration request {rid}")
            }
            Kind::Ack { upto_seq } if *upto_seq > self.last_seq => {
                bail!("cannot ack seq {upto_seq} beyond last seq {}", self.last_seq)
            }
            _ => {}
        }
        Ok(())
    }

    pub fn apply(&mut self, e: &Event) -> Result<()> {
        if e.seq <= self.last_seq {
            bail!("seq {} not after {}", e.seq, self.last_seq);
        }
        self.validate(e.by, &e.kind)?;
        self.last_seq = e.seq;
        match &e.kind {
            Kind::DecisionAdd { id, title, context, options } => self.decisions.push(Decision {
                id: id.clone(),
                title: title.clone(),
                context: context.clone(),
                options: options.clone(),
                status: DecisionStatus::Pending,
            }),
            Kind::DecisionResolve { id, text } => {
                self.decision_mut(id).status =
                    DecisionStatus::Resolved { text: text.clone(), seq: e.seq }
            }
            Kind::DecisionReopen { id } => self.decision_mut(id).status = DecisionStatus::Pending,
            Kind::DecisionWithdraw { id } => {
                self.decision_mut(id).status = DecisionStatus::Withdrawn
            }
            Kind::TodoAdd { id, title, context } => self.todos.push(Todo {
                id: id.clone(),
                title: title.clone(),
                context: context.clone(),
                done: None,
            }),
            Kind::TodoDone { id, note } => self.todo_mut(id).done = Some(note.clone()),
            Kind::TodoUndone { id } => self.todo_mut(id).done = None,
            Kind::ElabRequest { rid, target, selection, note } => self.elabs.push(Elab {
                rid: rid.clone(),
                target: target.clone(),
                selection: selection.clone(),
                note: note.clone(),
                answer: None,
            }),
            Kind::ElabAdd { rid, text_md } => {
                let r = self.elabs.iter_mut().find(|r| &r.rid == rid).unwrap();
                r.answer = Some(text_md.clone());
            }
            Kind::Ack { upto_seq } => self.acked_seq = self.acked_seq.max(*upto_seq),
        }
        Ok(())
    }

    // Callers have already validated that the id exists.
    fn decision_mut(&mut self, id: &str) -> &mut Decision {
        self.decisions.iter_mut().find(|d| d.id == id).unwrap()
    }

    fn todo_mut(&mut self, id: &str) -> &mut Todo {
        self.todos.iter_mut().find(|t| t.id == id).unwrap()
    }
}

/// Operator events the agent has not acked yet.
pub fn unprocessed<'a>(events: &'a [Event], state: &State) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|e| e.by == Actor::Operator && e.seq > state.acked_seq)
        .collect()
}
