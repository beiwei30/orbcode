use orbcode_app_server_protocol::{PlanOverview, SeedReadStateResult};
use orbcode_config::load_agent_definitions_with_warnings;
use orbcode_core::{CoreError, mcp_permission_target};
use orbcode_protocol::ProviderToolDefinition;
use orbcode_tools::{
    BackgroundTaskRecord, TaskListSnapshot, ToolCancellationToken, ToolContext,
    load_skill_definitions_with_bounded_mcp, workspace_plan_snapshot,
};

use std::sync::Arc;
use std::time::Duration;

use super::AppServer;
use super::advanced::advanced_capabilities;

const MCP_SKILL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);

impl AppServer {
    pub fn list_tools(&self) -> Vec<orbcode_tools::ToolSpec> {
        self.tools.planned().to_vec()
    }

    /// Seed the exact stale-write state used by normal Read/Edit/Write calls.
    pub async fn seed_read_state(
        &self,
        session_id: &str,
        path: &str,
        mtime: u64,
    ) -> Result<SeedReadStateResult, CoreError> {
        self.ensure_active_session(session_id)?;
        let config = self.sessions.effective_config();
        let requested = std::path::PathBuf::from(path);
        let requested = if requested.is_absolute() {
            requested
        } else {
            config.cwd.join(requested)
        };
        let canonical = tokio::fs::canonicalize(&requested).await.map_err(|error| {
            CoreError::Config(format!(
                "cannot seed missing or inaccessible file {}: {error}",
                requested.display()
            ))
        })?;

        let additional_directories = self.sessions.additional_directories();
        let mut allowed_roots = Vec::new();
        for root in std::iter::once(config.cwd.as_path()).chain(
            additional_directories
                .iter()
                .map(std::path::PathBuf::as_path),
        ) {
            if let Ok(root) = tokio::fs::canonicalize(root).await
                && !allowed_roots.iter().any(|existing| existing == &root)
            {
                allowed_roots.push(root);
            }
        }
        if !allowed_roots.iter().any(|root| canonical.starts_with(root)) {
            return Err(CoreError::PermissionDenied(format!(
                "read-state path is outside the session workspace: {}",
                canonical.display()
            )));
        }

        self.read_state
            .seed_current_file(&requested, u128::from(mtime))
            .await
            .map_err(CoreError::Config)?;
        Ok(SeedReadStateResult {
            session_id: session_id.to_string(),
            path: requested,
            mtime,
            seeded: true,
        })
    }

    pub async fn list_diagnostic_tools(&self) -> Vec<ProviderToolDefinition> {
        self.tools
            .diagnostic_definitions_with_mcp(true, true, &self.mcp)
            .await
    }

    pub(crate) fn tool_context_for_current_permissions(
        &self,
        input: &str,
        name: &str,
    ) -> ToolContext {
        let permissions = self.sessions.permission_context();
        let rule_allows_tool = permissions.tool_allowed_without_prompt(name, input);
        let config = self.sessions.effective_config();
        ToolContext {
            cwd: config.cwd.clone(),
            additional_directories: self.sessions.additional_directories(),
            home_dir: config.home_dir.clone(),
            sandbox_mode: config.sandbox_mode,
            sandbox_allow_network: config.sandbox_allow_network,
            allow_network: permissions.allow_network || rule_allows_tool,
            allow_tools: permissions.allow_tools || rule_allows_tool,
            mcp: self.mcp.clone(),
            progress: None,
            cancellation: ToolCancellationToken::default(),
            read_state: Some(Arc::clone(&self.read_state)),
            session_id: None,
            local_shell_tasks: Some(self.sessions.local_shell_tasks().clone()),
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: config.settings.env.clone(),
            skill_definitions: None,
        }
    }

    pub async fn plan_overview(&self) -> Result<PlanOverview, CoreError> {
        let context = self.tool_context_for_current_permissions("{}", "verify-plan-execution");
        let snapshot = workspace_plan_snapshot(&context)
            .await
            .map_err(CoreError::from)?;
        Ok(PlanOverview {
            plan_file: snapshot.plan_file,
            state_file: snapshot.state_file,
            in_plan_mode: snapshot.in_plan_mode,
            plan: snapshot.plan,
        })
    }

    pub async fn enter_plan_mode(&self) -> Result<orbcode_tools::ToolOutcome, CoreError> {
        self.invoke_tool("enter-plan-mode", "{}").await
    }

    pub async fn invoke_tool(
        &self,
        name: &str,
        input: impl Into<String>,
    ) -> Result<orbcode_tools::ToolOutcome, CoreError> {
        let input = input.into();
        let permissions = self.sessions.permission_context();
        if permissions.tool_denied(name, &input).is_some() {
            let denied_tool =
                mcp_permission_target(name, &input).unwrap_or_else(|| name.to_string());
            return Err(CoreError::PermissionDenied(format!(
                "permission denied for tool `{denied_tool}` by configured deny rule"
            )));
        }
        let mut context = self.tool_context_for_current_permissions(&input, name);
        if is_skill_tool(name) {
            context.skill_definitions = Some(self.skill_definitions().await);
        }
        self.tools
            .invoke(name, &input, &context)
            .await
            .map_err(CoreError::from)
    }

    pub async fn skill_definitions(&self) -> Vec<orbcode_tools::SkillDefinition> {
        let config = self.sessions.effective_config();
        load_skill_definitions_with_bounded_mcp(
            &config.home_dir,
            &config.cwd,
            &self.mcp,
            MCP_SKILL_DISCOVERY_TIMEOUT,
        )
        .await
        .unwrap_or_default()
    }

    pub async fn skill_definitions_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<orbcode_tools::SkillDefinition>, CoreError> {
        self.sessions
            .skill_definitions_for_session(session_id)
            .await
    }

    pub fn agent_definitions(&self) -> Vec<orbcode_config::AgentDefinition> {
        (*self.sessions.agent_definitions()).clone()
    }

    pub async fn agent_definitions_with_warnings(
        &self,
    ) -> (
        Vec<orbcode_config::AgentDefinition>,
        Vec<orbcode_config::AgentLoadWarning>,
    ) {
        let config = self.sessions.effective_config();
        match load_agent_definitions_with_warnings(&config.home_dir, &config.cwd).await {
            Ok(outcome) => (outcome.definitions, outcome.warnings),
            Err(_) => (self.agent_definitions(), Vec::new()),
        }
    }

    pub fn advanced_capabilities(&self) -> Vec<super::AdvancedCapability> {
        advanced_capabilities()
    }

    pub async fn load_task_list_snapshot(
        &self,
        task_list_id: &str,
    ) -> Result<TaskListSnapshot, CoreError> {
        let home_dir = &self.sessions.config().home_dir;
        orbcode_tools::load_task_list_snapshot(home_dir, task_list_id)
            .await
            .map_err(CoreError::from)
    }

    pub async fn list_local_agent_records_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<BackgroundTaskRecord>, CoreError> {
        let home_dir = &self.sessions.config().home_dir;
        orbcode_tools::list_local_agent_records_for_session(home_dir, session_id)
            .await
            .map_err(CoreError::from)
    }
}

fn is_skill_tool(name: &str) -> bool {
    name.eq_ignore_ascii_case("skill")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH};

    use orbcode_config::{
        AppConfigOverrides, PluginToolDefinition, load_plugin_registry, plugin_tool_definitions,
    };
    use orbcode_mcp::{McpAuth, McpServerConfig, McpServerStatus, McpServerTrust, McpTransport};
    use serde_json::json;

    use super::{AppServer, MCP_SKILL_DISCOVERY_TIMEOUT};

    const MCP_SKILL_BODY_MARKER: &str = "MCP_APP_SERVER_SKILL_BODY";

    fn test_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "orbcode-app-server-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn seeded_read_state_matches_file_tool_path_under_symlinked_cwd() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let real_cwd = root.path().join("real-workspace");
        let linked_cwd = root.path().join("linked-workspace");
        tokio::fs::create_dir_all(&home).await.expect("home");
        tokio::fs::create_dir_all(&real_cwd)
            .await
            .expect("real cwd");
        std::os::unix::fs::symlink(&real_cwd, &linked_cwd).expect("symlink cwd");

        let file = real_cwd.join("seeded.txt");
        tokio::fs::write(&file, "alpha\n").await.expect("seed file");
        let mtime = tokio::fs::metadata(&file)
            .await
            .expect("metadata")
            .modified()
            .expect("mtime")
            .duration_since(UNIX_EPOCH)
            .expect("after epoch")
            .as_millis() as u64;

        let app = AppServer::new(
            &linked_cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                allow_tools: Some(true),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        let bootstrap = app.bootstrap(None).await.expect("bootstrap");

        let seeded = app
            .seed_read_state(&bootstrap.session.session_id, "seeded.txt", mtime)
            .await
            .expect("seed read state");
        assert_eq!(seeded.path, linked_cwd.join("seeded.txt"));

        app.invoke_tool(
            "file-edit",
            json!({
                "file_path": "seeded.txt",
                "old_string": "alpha",
                "new_string": "omega"
            })
            .to_string(),
        )
        .await
        .expect("edit through symlinked cwd");
        assert_eq!(
            tokio::fs::read_to_string(file).await.expect("edited file"),
            "omega\n"
        );
    }

    struct FakeHttpResponse {
        body: String,
        delay: Option<StdDuration>,
    }

    impl FakeHttpResponse {
        fn ok(body: serde_json::Value) -> Self {
            Self {
                body: body.to_string(),
                delay: None,
            }
        }

        fn with_delay(mut self, delay: StdDuration) -> Self {
            self.delay = Some(delay);
            self
        }
    }

    fn spawn_fake_http_mcp_server(
        requests: usize,
        handler: impl Fn(String) -> FakeHttpResponse + Send + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake HTTP MCP server");
        let endpoint = format!("http://{}/mcp", listener.local_addr().expect("local addr"));
        thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().expect("accept fake HTTP request");
                stream
                    .set_read_timeout(Some(StdDuration::from_secs(2)))
                    .expect("set read timeout");
                let request = read_http_request(&mut stream);
                let response = handler(request);
                if let Some(delay) = response.delay {
                    thread::sleep(delay);
                }
                let payload = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.body.len(),
                    response.body
                );
                let _ = stream.write_all(payload.as_bytes());
            }
        });
        endpoint
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).expect("read fake HTTP request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n");
            if let Some(header_end) = header_end {
                let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
                let content_length = headers
                    .lines()
                    .find_map(|line| line.split_once(':'))
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let expected = header_end + 4 + content_length;
                while bytes.len() < expected {
                    let read = stream.read(&mut buffer).expect("read fake HTTP body");
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn mcp_server(id: &str, endpoint: String) -> McpServerConfig {
        McpServerConfig {
            id: id.to_string(),
            transport: McpTransport::Http,
            endpoint,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Test MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        }
    }

    fn docs_mcp_skill_server_config() -> McpServerConfig {
        let script = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  if [ -z "$id" ]; then id=1; fi
  case "$line" in
    *'"method":"initialize"'*)
      result='{"protocolVersion":"2024-11-05","capabilities":{"prompts":{}},"serverInfo":{"name":"docs","version":"0.1.0"}}'
      ;;
    *'"method":"prompts/list"'*)
      result='{"prompts":[{"name":"guide","description":"Use docs guide.","_meta":{"skill":true}},{"name":"plain","description":"Plain prompt."}]}'
      ;;
    *'"method":"prompts/get"'*)
      result='{"description":"Use docs guide.","messages":[{"role":"user","content":{"type":"text","text":"MCP_APP_SERVER_SKILL_BODY"}}]}'
      ;;
    *)
      result='{}'
      ;;
  esac
  printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done"#;

        McpServerConfig {
            id: "docs".to_string(),
            transport: McpTransport::Stdio,
            endpoint: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: BTreeMap::new(),
            cwd: None,
            headers: BTreeMap::new(),
            enabled: true,
            status: McpServerStatus::Ready,
            error: None,
            summary: "Docs MCP".to_string(),
            auth: McpAuth::None,
            trust: McpServerTrust::Trusted,
            transport_type_hint: None,
            source: None,
        }
    }

    fn json_rpc_id(request: &str) -> serde_json::Value {
        request
            .split("\r\n\r\n")
            .nth(1)
            .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
            .and_then(|value| value.get("id").cloned())
            .unwrap_or_else(|| json!(1))
    }

    #[tokio::test]
    async fn skill_definitions_include_trusted_mcp_prompt_skills() {
        let home = test_path("mcp-skills-home");
        let cwd = test_path("mcp-skills-cwd");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        let endpoint = spawn_fake_http_mcp_server(4, |request| {
            let id = json_rpc_id(&request);
            let result = if request.contains(r#""method":"initialize""#) {
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "prompts": {} },
                    "serverInfo": { "name": "fake-skills", "version": "0.1.0" }
                })
            } else if request.contains(r#""method":"prompts/list""#) {
                json!({
                    "prompts": [
                        {
                            "name": "guide",
                            "description": "List description",
                            "skill": true,
                            "arguments": []
                        },
                        {
                            "name": "ordinary",
                            "description": "Not a skill",
                            "arguments": []
                        }
                    ]
                })
            } else if request.contains(r#""method":"prompts/get""#) {
                json!({
                    "description": "Rendered description",
                    "messages": [{
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": "---\ndescription: Rendered Docs Guide\nwhen_to_use: Use when docs are needed\nallowed-tools: Read, Grep\n---\nUse the docs prompt body."
                        }
                    }]
                })
            } else {
                json!({})
            };
            FakeHttpResponse::ok(json!({"jsonrpc": "2.0", "id": id, "result": result}))
        });

        let app = AppServer::new(
            &cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        app.upsert_mcp_server(mcp_server("docs", endpoint))
            .await
            .expect("upsert mcp");

        let skills = app.skill_definitions().await;
        let skill = skills
            .iter()
            .find(|skill| skill.name == "docs:guide")
            .expect("mcp skill");

        assert_eq!(
            skill.source,
            orbcode_tools::SkillSource::Mcp {
                server_id: "docs".to_string()
            }
        );
        assert_eq!(skill.description.as_deref(), Some("Rendered Docs Guide"));
        assert_eq!(
            skill.when_to_use.as_deref(),
            Some("Use when docs are needed")
        );
        assert_eq!(skill.allowed_tools, vec!["Read", "Grep"]);
        assert_eq!(skill.body, "Use the docs prompt body.");
        assert!(
            skills.iter().all(|skill| skill.name != "docs:ordinary"),
            "unmarked MCP prompts must not become skills"
        );
    }

    #[tokio::test]
    async fn invoke_skill_loads_trusted_mcp_prompt_skill() {
        let home = test_path("mcp-skill-invoke-home");
        let cwd = test_path("mcp-skill-invoke-cwd");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");
        let endpoint = spawn_fake_http_mcp_server(4, |request| {
            let id = json_rpc_id(&request);
            let result = if request.contains(r#""method":"initialize""#) {
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "prompts": {} },
                    "serverInfo": { "name": "fake-skills", "version": "0.1.0" }
                })
            } else if request.contains(r#""method":"prompts/list""#) {
                json!({
                    "prompts": [{
                        "name": "guide",
                        "description": "List description",
                        "skill": true,
                        "arguments": []
                    }]
                })
            } else if request.contains(r#""method":"prompts/get""#) {
                json!({
                    "description": "Rendered description",
                    "messages": [{
                        "role": "user",
                        "content": {
                            "type": "text",
                            "text": "---\ndescription: Rendered Docs Guide\n---\nUse $ARGUMENTS from the MCP skill."
                        }
                    }]
                })
            } else {
                json!({})
            };
            FakeHttpResponse::ok(json!({"jsonrpc": "2.0", "id": id, "result": result}))
        });

        let app = AppServer::new(
            &cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        app.upsert_mcp_server(mcp_server("docs", endpoint))
            .await
            .expect("upsert mcp");

        let outcome = app
            .invoke_tool("Skill", r#"{"skill":"docs:guide","args":"runtime args"}"#)
            .await
            .expect("invoke skill");

        assert_eq!(outcome.summary, "Loaded skill `docs:guide`.");
        assert!(outcome.output.contains("Skill: docs:guide"));
        assert!(outcome.output.contains("Source: mcp"));
        assert!(
            outcome
                .output
                .contains("Use runtime args from the MCP skill.")
        );
    }

    #[tokio::test]
    async fn skill_definitions_keep_local_skills_when_mcp_prompt_loading_fails() {
        let home = test_path("mcp-skills-fail-home");
        let cwd = test_path("mcp-skills-fail-cwd");
        let skill_dir = home.join("skills").join("local");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&skill_dir)
            .await
            .expect("skill dir");
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Local helper\n---\nLocal body",
        )
        .await
        .expect("skill");

        let app = AppServer::new(
            &cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        app.upsert_mcp_server(mcp_server("offline", "http://127.0.0.1:1/mcp".to_string()))
            .await
            .expect("upsert mcp");

        let skills = app.skill_definitions().await;

        assert!(
            skills.iter().any(|skill| skill.name == "local"),
            "MCP loading failure must not drop local skills"
        );
    }

    #[tokio::test]
    async fn skill_definitions_return_local_skills_when_mcp_prompt_loading_is_slow() {
        let home = test_path("mcp-skills-slow-home");
        let cwd = test_path("mcp-skills-slow-cwd");
        let skill_dir = home.join("skills").join("local");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&skill_dir)
            .await
            .expect("skill dir");
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Local helper\n---\nLocal body",
        )
        .await
        .expect("skill");
        let endpoint = spawn_fake_http_mcp_server(2, |request| {
            let id = json_rpc_id(&request);
            let result = if request.contains(r#""method":"initialize""#) {
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "prompts": {} },
                    "serverInfo": { "name": "slow-skills", "version": "0.1.0" }
                })
            } else {
                json!({"prompts": []})
            };
            let response =
                FakeHttpResponse::ok(json!({"jsonrpc": "2.0", "id": id, "result": result}));
            if request.contains(r#""method":"prompts/list""#) {
                response.with_delay(MCP_SKILL_DISCOVERY_TIMEOUT + StdDuration::from_millis(500))
            } else {
                response
            }
        });

        let app = AppServer::new(
            &cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        app.upsert_mcp_server(mcp_server("slow", endpoint))
            .await
            .expect("upsert mcp");

        let started = Instant::now();
        let skills = app.skill_definitions().await;

        assert!(
            started.elapsed() < MCP_SKILL_DISCOVERY_TIMEOUT + StdDuration::from_millis(250),
            "skill_definitions should return after the MCP skill timeout"
        );
        assert!(
            skills.iter().any(|skill| skill.name == "local"),
            "slow MCP loading must not drop local skills"
        );
    }

    #[tokio::test]
    async fn plugin_tool_definitions_flow_to_provider_tool_definitions() {
        let home = test_path("plugin-tools-home");
        let cwd = test_path("plugin-tools-cwd");
        let plugin_root = home.join("plugin-cache").join("demo").join("1.0.0");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(plugin_root.join(".claude-plugin"))
            .await
            .expect("plugin manifest dir");
        tokio::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{
                "name": "demo",
                "tools": [
                    {
                        "name": "demo_search",
                        "description": "Search the demo index",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "query": { "type": "string" } },
                            "required": ["query"]
                        },
                        "requiresPermission": false
                    },
                    {
                        "name": "demo_write",
                        "description": "Write to the demo index"
                    }
                ]
            }"#,
        )
        .await
        .expect("plugin manifest");
        tokio::fs::create_dir_all(home.join("plugins"))
            .await
            .expect("plugins dir");
        tokio::fs::write(
            home.join("plugins").join("installed_plugins.json"),
            format!(
                r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
                plugin_root.display()
            ),
        )
        .await
        .expect("installed plugins");
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await
        .expect("settings");

        let registry = load_plugin_registry(&home, &cwd).await.expect("registry");
        let tools = plugin_tool_definitions(&registry);

        assert_eq!(tools.len(), 2, "both tools should be parsed");
        assert_eq!(tools[0].name, "demo_search");
        assert!(!tools[0].requires_permission);
        assert_eq!(tools[1].name, "demo_write");
        assert!(tools[1].requires_permission, "default is true");

        let registry = orbcode_tools::ToolRegistry::foundation();
        registry.set_plugin_tools(&tools);

        let provider_defs = registry.dynamic_definitions();
        assert_eq!(provider_defs.len(), 2);
        assert_eq!(provider_defs[0].name, "plugin__demo__demo_search");
        assert!(provider_defs[0].input_schema.get("properties").is_some());
        assert_eq!(provider_defs[1].name, "plugin__demo__demo_write");
        assert_eq!(
            provider_defs[1].input_schema,
            serde_json::json!({"type": "object"}),
        );
    }

    #[tokio::test]
    async fn skill_definitions_and_invoke_include_trusted_mcp_prompt_skills() {
        let home = test_path("mcp-skill-home");
        let cwd = test_path("mcp-skill-cwd");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");

        let app = AppServer::new(
            &cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                allow_tools: Some(true),
                allow_network: Some(true),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");
        app.mcp
            .upsert_server(docs_mcp_skill_server_config())
            .await
            .expect("register docs MCP");

        let skills = app.skill_definitions().await;
        let skill = skills
            .iter()
            .find(|skill| skill.name == "docs:guide")
            .expect("MCP prompt skill is listed");
        assert!(
            !skills.iter().any(|skill| skill.name == "docs:plain"),
            "unmarked MCP prompt must not be exposed as a skill"
        );
        assert!(
            skill.body.contains(MCP_SKILL_BODY_MARKER),
            "MCP skill body should come from prompts/get: {}",
            skill.body
        );

        let outcome = app
            .invoke_tool("Skill", r#"{"skill":"docs:guide","args":"topic"}"#)
            .await
            .expect("invoke MCP-backed Skill");
        assert!(
            outcome.output.contains(MCP_SKILL_BODY_MARKER),
            "Skill invoke should use MCP-aware definitions: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn list_diagnostic_tools_includes_hidden_and_plugin_tools() {
        let home = test_path("diag-tools-home");
        let cwd = test_path("diag-tools-cwd");
        tokio::fs::create_dir_all(&cwd).await.expect("cwd");
        tokio::fs::create_dir_all(&home).await.expect("home");

        let app = AppServer::new(
            &cwd,
            AppConfigOverrides {
                home_dir: Some(home),
                ..AppConfigOverrides::default()
            },
        )
        .await
        .expect("app server");

        app.tools.set_plugin_tools(&[PluginToolDefinition {
            name: "inspect".into(),
            description: "Plugin inspect tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
            requires_permission: false,
            plugin_id: "diag@market".into(),
            plugin_name: "diag".into(),
        }]);

        let diag = app.list_diagnostic_tools().await;
        let names: std::collections::HashSet<String> =
            diag.iter().map(|d| d.name.clone()).collect();

        assert!(
            names.contains("AskUserQuestion"),
            "diagnostic listing must include provider-hidden ask-user-question"
        );
        assert!(
            names.contains("plugin__diag__inspect"),
            "diagnostic listing must include plugin tools"
        );
        assert!(
            names.contains("Bash"),
            "diagnostic listing must include foundation tools"
        );
    }
}
