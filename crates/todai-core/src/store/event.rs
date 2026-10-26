use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    Agent,
    Operator,
}

/// One line of the log. `seq` is strictly increasing and assigned under the file lock.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub seq: u64,
    /// Unix time in milliseconds.
    pub ts: u64,
    pub by: Actor,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum Kind {
    #[serde(rename = "decision.add")]
    DecisionAdd {
        id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
    },
    #[serde(rename = "decision.resolve")]
    DecisionResolve { id: String, text: String },
    #[serde(rename = "decision.reopen")]
    DecisionReopen { id: String },
    #[serde(rename = "decision.withdraw")]
    DecisionWithdraw { id: String },

    #[serde(rename = "todo.add")]
    TodoAdd {
        id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
    #[serde(rename = "todo.done")]
    TodoDone {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "todo.undone")]
    TodoUndone { id: String },

    #[serde(rename = "elab.request")]
    ElabRequest {
        rid: String,
        target: String,
        selection: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        /// The elaboration this one follows up on; they form a thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    #[serde(rename = "elab.add")]
    ElabAdd { rid: String, text_md: String },

    /// The operator removed finished items (and their elaborations) from the board.
    #[serde(rename = "item.trash")]
    Trash { ids: Vec<String> },

    /// The agent has consumed operator events up to and including `upto_seq`.
    #[serde(rename = "ack")]
    Ack { upto_seq: u64 },
}

impl Kind {
    /// Which actor may emit this kind. `None` means either.
    pub fn allowed_actor(&self) -> Option<Actor> {
        match self {
            Kind::DecisionAdd { .. }
            | Kind::DecisionWithdraw { .. }
            | Kind::TodoAdd { .. }
            | Kind::ElabAdd { .. }
            | Kind::Ack { .. } => Some(Actor::Agent),
            Kind::DecisionResolve { .. }
            | Kind::DecisionReopen { .. }
            | Kind::TodoDone { .. }
            | Kind::TodoUndone { .. }
            | Kind::ElabRequest { .. }
            | Kind::Trash { .. } => Some(Actor::Operator),
        }
    }
}
