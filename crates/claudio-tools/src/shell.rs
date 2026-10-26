//! Outil PowerShell : chaque commande est montrée à l'utilisateur et confirmée ;
//! elle s'exécute par défaut dans le bac à sable (voir `claudio_perms::sandbox`).

use std::time::Duration;

use anyhow::Result;
use claudio_perms::Action;
use claudio_perms::sandbox::{self, ShellRequest};
use serde_json::{Value, json};

use crate::{Context, Done, ToolOutput, ToolSpec, arg_path, arg_str, untrusted};

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 600;

pub fn specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "run_powershell".into(),
        description:
            "Exécute une commande PowerShell (Windows) après confirmation de l'utilisateur. \
            En bac à sable, elle ne peut écrire que dans l'espace de travail de Claudio. \
            Préférer les outils fichiers quand ils suffisent."
                .into(),
        parameters: json!({"type": "object", "properties": {
            "command": {"type": "string", "description": "Script PowerShell"},
            "cwd": {"type": "string", "description": "Dossier de travail (par défaut : l'espace de travail)"},
            "timeout_seconds": {"type": "integer", "description": "Délai maximal (120 par défaut, 600 au plus)"}
        }, "required": ["command"]}),
    }]
}

pub(crate) async fn call(ctx: &Context<'_>, name: &str, args: &Value) -> Option<Result<Done>> {
    (name == "run_powershell").then_some(())?;
    Some(run(ctx, args).await)
}

async fn run(ctx: &Context<'_>, args: &Value) -> Result<Done> {
    let command = arg_str(args, "command")?.to_owned();
    let workspace = ctx.engine.workspace().to_path_buf();
    let cwd = match args
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        Some(_) => arg_path(args, "cwd")?,
        None => workspace.clone(),
    };
    let sandboxed = ctx.engine.policy().shell_sandbox;
    // Le dossier de travail doit être lisible, puis la commande elle-même est confirmée.
    let d1 = ctx
        .authorize("run_powershell", Action::Read(cwd.clone()))
        .await?;
    let d2 = ctx
        .authorize(
            "run_powershell",
            Action::Shell {
                command: command.clone(),
                cwd: cwd.clone(),
                sandboxed,
            },
        )
        .await?;
    let timeout = args
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT)
        .clamp(1, MAX_TIMEOUT);
    let req = ShellRequest {
        script: command.clone(),
        cwd,
        sandboxed,
        timeout: Duration::from_secs(timeout),
        temp_dir: workspace.join(".tmp"),
    };
    let out = tokio::task::spawn_blocking(move || sandbox::run(&req)).await??;
    let mut text = String::new();
    if out.timed_out {
        text.push_str(&format!("[arrêtée après {timeout} s]\n"));
    }
    if let Some(code) = out.exit_code {
        text.push_str(&format!("[code de sortie {code}]\n"));
    }
    if !out.stdout.trim().is_empty() {
        text.push_str(&out.stdout);
    }
    if !out.stderr.trim().is_empty() {
        text.push_str("\n[erreurs]\n");
        text.push_str(&out.stderr);
    }
    let failed = out.timed_out || out.exit_code.is_some_and(|c| c != 0);
    let body = untrusted("sortie de commande", text.trim());
    Ok(Done {
        output: if failed {
            ToolOutput::error(body)
        } else {
            ToolOutput::ok(body)
        },
        detail: format!(
            "{command}{}",
            if sandboxed {
                ""
            } else {
                " (sans bac à sable)"
            }
        ),
        decision: format!("{d1} / {d2}"),
    })
}
