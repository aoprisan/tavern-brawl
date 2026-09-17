//! The real referee: one `system_one` call per move through the `typesafe` SDK.

use std::time::{Duration, Instant};

use typesafe::{Client, RetryPolicy, SystemOneResponse};

use crate::game::{PlayerName, Target};

use super::{MoveState, Referee, RefereeError, Verdict, questions};

pub struct JevReferee {
    client: Client,
}

impl JevReferee {
    /// Per-attempt timeout. A 60-second round cannot wait for the SDK's default 10 s.
    pub const TIMEOUT: Duration = Duration::from_secs(4);
    /// Total budget per judgment, retries and backoff included.
    pub const BUDGET: Duration = Duration::from_secs(8);

    /// Client from the environment (`TYPESAFE_API_KEY`, optional `TYPESAFE_BASE_URL`,
    /// `TYPESAFE_DEFAULT_MODEL`), with a demo-friendly retry policy.
    pub fn from_env() -> Result<Self, RefereeError> {
        let client = Client::builder()
            .timeout(Self::TIMEOUT)
            .retry(Self::retry_policy())
            .build()?;
        Ok(Self { client })
    }

    /// Wrap an already-configured client (tests, custom base URLs).
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// The SDK default (408/429/5xx incl. 529, connection errors, timeouts, `Retry-After`)
    /// with faster backoff and a short total budget.
    pub fn retry_policy() -> RetryPolicy {
        RetryPolicy::default()
            .max_retries(2)
            .backoff(Duration::from_millis(200), Duration::from_secs(1))
            .budget(Some(Self::BUDGET))
    }

    pub fn model(&self) -> &str {
        self.client.default_model()
    }

    /// Map a response to a verdict. Pure, so it can be tested without a client.
    pub fn verdict_from(
        res: &SystemOneResponse,
        latency: Duration,
    ) -> Result<Verdict, RefereeError> {
        let missing = |q: &str| RefereeError::BadAnswer(format!("no answer for question {q:?}"));
        let kind = res.choice("kind").ok_or_else(|| missing("kind"))?;
        let target = res.choice("target").ok_or_else(|| missing("target"))?;
        let force = res.score("force").ok_or_else(|| missing("force"))?;
        let breaks_rules = res
            .noul("breaks_rules")
            .ok_or_else(|| missing("breaks_rules"))?;

        Ok(Verdict {
            kind: kind.parse().map_err(RefereeError::BadAnswer)?,
            kind_confidence: kind.confidence as f32,
            target: Target::from_label(&target.choice),
            target_confidence: target.confidence as f32,
            force: force.score as f32,
            force_confidence: force.confidence as f32,
            breaks_rules: breaks_rules.noul as f32,
            latency,
            input_tokens: res.usage.input_tokens.unwrap_or(0),
        })
    }
}

#[async_trait::async_trait]
impl Referee for JevReferee {
    async fn judge(
        &self,
        state: &MoveState,
        players: &[PlayerName],
    ) -> Result<Verdict, RefereeError> {
        let started = Instant::now();
        let res = self.client.system_one(state, questions(players)).await?;
        let latency = started.elapsed();
        tracing::debug!(
            request_id = res.request_id().unwrap_or("-"),
            attempts = res.meta.attempts,
            input_tokens = ?res.usage.input_tokens,
            latency_ms = latency.as_millis() as u64,
            model = %res.model,
            "judged"
        );
        Self::verdict_from(&res, latency)
    }

    fn name(&self) -> &'static str {
        "Jev via typesafe-ai-sdk"
    }
}
