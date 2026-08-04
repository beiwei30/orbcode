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

    pub async fn start_chatgpt_browser_login(
        &self,
    ) -> Result<orbcode_config::ChatGptBrowserLoginSession, CoreError> {
        Ok(self.auth.start_chatgpt_browser_login().await?)
    }

    pub async fn complete_chatgpt_browser_login(
        &self,
        session: orbcode_config::ChatGptBrowserLoginSession,
    ) -> Result<AuthStatusEntry, CoreError> {
        Ok(self.auth.complete_chatgpt_browser_login(session).await?)
    }

    pub async fn start_chatgpt_device_login(
        &self,
    ) -> Result<orbcode_config::ChatGptDeviceLoginSession, CoreError> {
        Ok(self.auth.start_chatgpt_device_login().await?)
    }

    pub async fn complete_chatgpt_device_login(
        &self,
        session: orbcode_config::ChatGptDeviceLoginSession,
    ) -> Result<AuthStatusEntry, CoreError> {
        Ok(self.auth.complete_chatgpt_device_login(session).await?)
    }
}
