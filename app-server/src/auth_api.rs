use orbcode_config::{AuthMethod, AuthOverview, AuthStatusEntry};
use orbcode_core::CoreError;
use orbcode_protocol::ProviderId;

use super::AppServer;

impl AppServer {
    pub async fn auth_overview(&self) -> Result<AuthOverview, CoreError> {
        Ok(self.auth.overview().await?)
    }

    pub async fn auth_login(
        &self,
        provider: ProviderId,
        method: AuthMethod,
        token: Option<String>,
        env_var: Option<String>,
    ) -> Result<AuthStatusEntry, CoreError> {
        Ok(self.auth.login(provider, method, token, env_var).await?)
    }

    pub async fn auth_logout(&self, provider: Option<ProviderId>) -> Result<usize, CoreError> {
        Ok(self.auth.logout(provider).await?)
    }
}
