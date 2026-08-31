use gray_session::{JsonlSessionStore, SessionId};

pub(crate) struct SessionState {
    pub(crate) store: JsonlSessionStore,
    pub(crate) session_id: SessionId,
}
