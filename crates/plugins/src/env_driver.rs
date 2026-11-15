//! The environment a plugin description pins, applied where every session passes through.
//!
//! `[env]` in an agent description (`docs/DESIGN.md` §9.3) is how two entries backed by one
//! CLI stay different: a site (`CODEBUDDY_INTERNET_ENVIRONMENT`) or a state directory
//! (`DSH_HOME`). It has to reach *every* way a session starts — a new conversation, a
//! resumed one, an adopted process, the self-check probe, a session on a remote node — not
//! just the main path. Wrapping the driver once, at [`crate::Profile::resolve`], is the only
//! place all of those go through; doing it per call site had already missed three of four.
//!
//! Order: the plugin's variables first, then the agent's own and its account's, which win on
//! a name they both set.

use std::sync::Arc;

use async_trait::async_trait;
use crossagents_core::{
    Capabilities, CliStatus, CoreError, Driver, DriverKind, DriverSession, SessionSpec,
};

pub struct EnvDriver {
    inner: Arc<dyn Driver>,
    /// Already expanded (`%VAR%`, leading `~`) by [`crate::PluginConfig::env_expanded`].
    env: Vec<(String, String)>,
}

impl EnvDriver {
    /// Wraps `inner`, or hands it back as is when the description pins nothing.
    pub fn wrap(inner: Arc<dyn Driver>, env: Vec<(String, String)>) -> Arc<dyn Driver> {
        if env.is_empty() {
            return inner;
        }
        Arc::new(Self { inner, env })
    }

    fn with_env(&self, mut spec: SessionSpec) -> SessionSpec {
        let mut env = self.env.clone();
        for (key, value) in spec.env {
            match env.iter_mut().find(|(kept, _)| kept.eq_ignore_ascii_case(&key)) {
                Some(slot) => slot.1 = value,
                None => env.push((key, value)),
            }
        }
        spec.env = env;
        spec
    }
}

#[async_trait]
impl Driver for EnvDriver {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn kind(&self) -> DriverKind {
        self.inner.kind()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn models(&self) -> Vec<String> {
        self.inner.models()
    }

    fn model_efforts(&self, model: &str) -> Option<Vec<String>> {
        self.inner.model_efforts(model)
    }

    fn installed(&self) -> bool {
        self.inner.installed()
    }

    async fn cli_status(&self) -> Option<CliStatus> {
        self.inner.cli_status().await
    }

    async fn probe_cli(&self) {
        self.inner.probe_cli().await;
    }

    fn refresh_cli(&self) {
        self.inner.refresh_cli();
    }

    fn cli_places(&self) -> Vec<String> {
        self.inner.cli_places()
    }

    async fn open(&self, spec: SessionSpec) -> Result<Box<dyn DriverSession>, CoreError> {
        self.inner.open(self.with_env(spec)).await
    }

    async fn resume(
        &self,
        session_id: &str,
        spec: SessionSpec,
    ) -> Result<Box<dyn DriverSession>, CoreError> {
        self.inner.resume(session_id, self.with_env(spec)).await
    }

    async fn adopt(
        &self,
        session_id: &str,
        turn_running: bool,
        spec: SessionSpec,
    ) -> Result<Option<Box<dyn DriverSession>>, CoreError> {
        self.inner
            .adopt(session_id, turn_running, self.with_env(spec))
            .await
    }
}
