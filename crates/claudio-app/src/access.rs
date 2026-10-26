//! Accès des outils : dossiers autorisés, règles, serveurs MCP et journal d'audit.

use std::path::Path;

use anyhow::{Context, Result, bail};
use claudio_core::audit::{AuditEntry, StoredGrant};
use claudio_core::config::{McpServerConfig, McpTransport, Permissions};
use claudio_tools::mcp::McpTool;
use serde::Serialize;

use crate::Claudio;

#[derive(Debug, Clone, Serialize)]
pub struct AccessOverview {
    pub workspace: String,
    pub grants: Vec<StoredGrant>,
    pub permissions: Permissions,
    pub tools_enabled: bool,
    pub mcp_servers: Vec<McpServerConfig>,
}

impl Claudio {
    pub fn access_overview(&self) -> Result<AccessOverview> {
        let cfg = self.config();
        Ok(AccessOverview {
            workspace: self.perms.workspace().display().to_string(),
            grants: self.db.grants()?,
            permissions: cfg.permissions,
            tools_enabled: cfg.tools_enabled,
            mcp_servers: cfg.mcp_servers,
        })
    }

    /// Dossier choisi par l'utilisateur dans l'interface.
    pub fn grant_folder(&self, path: &Path, write: bool) -> Result<()> {
        self.perms.grant_folder(path, write)
    }

    pub fn revoke_grant(&self, id: i64) -> Result<()> {
        self.db.remove_grant(id)
    }

    pub fn set_permissions(&self, p: Permissions) -> Result<()> {
        self.update_config(|c| c.permissions = p.clone())?;
        self.perms.set_policy(p);
        Ok(())
    }

    pub fn set_tools_enabled(&self, on: bool) -> Result<()> {
        self.update_config(|c| c.tools_enabled = on)?;
        Ok(())
    }

    pub fn audit_log(&self, limit: u32, before: Option<i64>) -> Result<Vec<AuditEntry>> {
        self.db.audit_entries(limit.clamp(1, 500), before)
    }

    /// `None` si le journal est intact, sinon la première entrée altérée.
    pub fn verify_audit(&self) -> Result<Option<i64>> {
        self.db.verify_audit()
    }

    fn validate_transport(t: &McpTransport) -> Result<()> {
        match t {
            McpTransport::Stdio { command, .. } if command.trim().is_empty() => {
                bail!("indiquez la commande qui lance le serveur")
            }
            McpTransport::Http { url } => claudio_llm::providers::validate_base_url(url),
            _ => Ok(()),
        }
    }

    pub fn add_mcp_server(&self, name: &str, transport: McpTransport) -> Result<McpServerConfig> {
        Self::validate_transport(&transport)?;
        let cfg = self.config();
        let id = (1..)
            .map(|n| format!("mcp{n}"))
            .find(|id| !cfg.mcp_servers.iter().any(|s| &s.id == id))
            .expect("identifiant libre");
        let server = McpServerConfig {
            id,
            name: match name.trim() {
                "" => "Serveur MCP".to_owned(),
                n => n.to_owned(),
            },
            transport,
            enabled: true,
        };
        self.update_config(|c| c.mcp_servers.push(server.clone()))?;
        Ok(server)
    }

    pub fn set_mcp_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.update_config(|c| {
            if let Some(s) = c.mcp_servers.iter_mut().find(|s| s.id == id) {
                s.enabled = enabled;
            }
        })?;
        Ok(())
    }

    pub async fn remove_mcp_server(&self, id: &str) -> Result<()> {
        let cfg = self.update_config(|c| c.mcp_servers.retain(|s| s.id != id))?;
        self.toolbox.mcp.configure(cfg.mcp_servers).await;
        Ok(())
    }

    /// Lance (ou contacte) le serveur et liste ses outils.
    pub async fn test_mcp_server(&self, id: &str) -> Result<Vec<McpTool>> {
        let cfg = self.config();
        let server = cfg
            .mcp_servers
            .iter()
            .find(|s| s.id == id)
            .context("serveur MCP inconnu")?;
        self.toolbox.mcp.test(server).await
    }
}
