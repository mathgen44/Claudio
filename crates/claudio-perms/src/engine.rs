//! Moteur de permissions : décide si une action d'outil est permise, doit être
//! confirmée par l'utilisateur, ou est refusée.
//!
//! Principe : rien n'est accessible par défaut. L'utilisateur accorde des dossiers
//! (lecture ou lecture/écriture), des sites et des outils MCP, pour une fois, pour
//! la conversation ou de façon permanente. Le modèle ne peut que *demander* : seule
//! une réponse de l'utilisateur élargit les droits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use anyhow::Result;
use claudio_core::Database;
use claudio_core::config::{Permissions, Policy};
use serde::{Deserialize, Serialize};

use crate::paths::{folder_of, is_within, normalize};

/// Ce qu'un outil veut faire.
#[derive(Debug, Clone)]
pub enum Action {
    Read(PathBuf),
    /// Écrire un fichier ; `overwrite` s'il existe déjà.
    Write {
        path: PathBuf,
        overwrite: bool,
    },
    Delete(PathBuf),
    Fetch {
        url: url::Url,
    },
    Shell {
        command: String,
        cwd: PathBuf,
        sandboxed: bool,
    },
    Mcp {
        server: String,
        tool: String,
        args: String,
    },
}

/// Durée d'une autorisation donnée par l'utilisateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Deny,
    Once,
    Session,
    Always,
}

/// Question à poser à l'utilisateur.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Approval {
    pub title: String,
    /// Détail exact de l'action (chemin, adresse complète, commande).
    pub detail: String,
    /// Durées proposées en plus du refus.
    pub options: Vec<Answer>,
    /// Action irréversible ou sensible : à mettre en évidence.
    pub dangerous: bool,
}

#[derive(Debug, Clone)]
pub enum Check {
    Allowed(String),
    Ask(Approval),
    Denied(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Grant {
    Folder {
        path: PathBuf,
        write: bool,
    },
    /// Écrasement de fichiers dans un dossier (conversation seulement).
    Overwrite(PathBuf),
    Host(String),
    McpTool {
        server: String,
        tool: String,
    },
}

/// Emplacements interdits même si un dossier parent est autorisé.
#[derive(Debug, Clone, Default)]
pub struct Protected {
    /// Ni lecture ni écriture (données de Claudio, clés SSH, profils de navigateurs…).
    pub no_access: Vec<PathBuf>,
    /// Lecture possible, écriture interdite (Windows, Program Files).
    pub read_only: Vec<PathBuf>,
}

impl Protected {
    /// Liste par défaut pour l'utilisateur courant.
    pub fn defaults(claudio_dirs: &[PathBuf]) -> Self {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let mut no_access: Vec<PathBuf> = claudio_dirs.to_vec();
        if let Some(home) = env("USERPROFILE") {
            for d in [".ssh", ".aws", ".azure", ".gnupg", ".kube", ".docker"] {
                no_access.push(home.join(d));
            }
        }
        if let Some(roaming) = env("APPDATA") {
            for d in [
                r"Microsoft\Credentials",
                r"Microsoft\Protect",
                r"Mozilla\Firefox\Profiles",
            ] {
                no_access.push(roaming.join(d));
            }
        }
        if let Some(local) = env("LOCALAPPDATA") {
            for d in [
                r"Microsoft\Credentials",
                r"Google\Chrome\User Data",
                r"Microsoft\Edge\User Data",
                r"BraveSoftware\Brave-Browser\User Data",
            ] {
                no_access.push(local.join(d));
            }
        }
        let read_only = [
            "SystemRoot",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
        ]
        .iter()
        .filter_map(|k| env(k))
        .collect();
        Self {
            no_access: canon_all(no_access),
            read_only: canon_all(read_only),
        }
    }
}

fn canon_all(v: Vec<PathBuf>) -> Vec<PathBuf> {
    v.into_iter()
        .map(|p| dunce::canonicalize(&p).unwrap_or(p))
        .collect()
}

pub struct Engine {
    db: Database,
    policy: RwLock<Permissions>,
    protected: Protected,
    /// Espace de travail de Claudio : toujours accessible en lecture/écriture.
    workspace: PathBuf,
    session: Mutex<HashMap<String, Vec<Grant>>>,
}

impl Engine {
    pub fn new(
        db: Database,
        policy: Permissions,
        protected: Protected,
        workspace: PathBuf,
    ) -> Self {
        Self {
            db,
            policy: RwLock::new(policy),
            protected,
            workspace: dunce::canonicalize(&workspace).unwrap_or(workspace),
            session: Mutex::default(),
        }
    }

    pub fn set_policy(&self, p: Permissions) {
        *self.policy.write().unwrap() = p;
    }

    pub fn policy(&self) -> Permissions {
        self.policy.read().unwrap().clone()
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn grants(&self, session: &str) -> Vec<Grant> {
        let mut out: Vec<Grant> = self
            .db
            .grants()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|g| match g.kind.as_str() {
                "folder" => Some(Grant::Folder {
                    path: PathBuf::from(g.target),
                    write: g.write,
                }),
                "host" => Some(Grant::Host(g.target)),
                "mcp" => g.target.split_once('/').map(|(s, t)| Grant::McpTool {
                    server: s.to_owned(),
                    tool: t.to_owned(),
                }),
                _ => None,
            })
            .collect();
        if let Some(s) = self.session.lock().unwrap().get(session) {
            out.extend(s.iter().cloned());
        }
        out
    }

    fn folder_access(&self, path: &Path, session: &str) -> Option<bool> {
        if is_within(path, &self.workspace) {
            return Some(true);
        }
        self.grants(session)
            .iter()
            .filter_map(|g| match g {
                Grant::Folder { path: root, write } if is_within(path, root) => Some(*write),
                _ => None,
            })
            .max()
    }

    fn protection(&self, path: &Path, write: bool) -> Option<String> {
        if is_within(path, &self.workspace) {
            return None;
        }
        if self.protected.no_access.iter().any(|p| is_within(path, p)) {
            return Some(format!("{} est un emplacement protégé", path.display()));
        }
        if write && self.protected.read_only.iter().any(|p| is_within(path, p)) {
            return Some(format!(
                "{} est en lecture seule (dossier système)",
                path.display()
            ));
        }
        None
    }

    /// Évalue une action pour une conversation.
    pub fn check(&self, action: &Action, session: &str) -> Result<Check> {
        let policy = self.policy();
        Ok(match action {
            Action::Read(p) => {
                let p = normalize(p)?;
                if let Some(why) = self.protection(&p, false) {
                    return Ok(Check::Denied(why));
                }
                match self.folder_access(&p, session) {
                    Some(_) => Check::Allowed("dossier autorisé".into()),
                    None => Check::Ask(Approval {
                        title: "Lire des fichiers".into(),
                        detail: folder_of(&p).display().to_string(),
                        options: vec![Answer::Once, Answer::Session, Answer::Always],
                        dangerous: false,
                    }),
                }
            }
            Action::Write { path, overwrite } => {
                let p = normalize(path)?;
                if let Some(why) = self.protection(&p, true) {
                    return Ok(Check::Denied(why));
                }
                let can_write = self.folder_access(&p, session) == Some(true);
                let overwrite_ok = is_within(&p, &self.workspace)
                    || self
                        .grants(session)
                        .iter()
                        .any(|g| matches!(g, Grant::Overwrite(root) if is_within(&p, root)));
                if can_write && (!overwrite || overwrite_ok) {
                    Check::Allowed("dossier autorisé en écriture".into())
                } else if *overwrite && can_write {
                    Check::Ask(Approval {
                        title: "Remplacer un fichier existant (une copie de sauvegarde est gardée)"
                            .into(),
                        detail: p.display().to_string(),
                        options: vec![Answer::Once, Answer::Session],
                        dangerous: true,
                    })
                } else {
                    Check::Ask(Approval {
                        title: if *overwrite {
                            "Remplacer un fichier existant (une copie de sauvegarde est gardée)"
                                .into()
                        } else {
                            "Écrire dans un dossier".into()
                        },
                        detail: if *overwrite {
                            p.display().to_string()
                        } else {
                            folder_of(&p).display().to_string()
                        },
                        options: vec![Answer::Once, Answer::Session, Answer::Always],
                        dangerous: *overwrite,
                    })
                }
            }
            Action::Delete(p) => {
                let p = normalize(p)?;
                if let Some(why) = self.protection(&p, true) {
                    return Ok(Check::Denied(why));
                }
                if is_within(&p, &self.workspace) {
                    return Ok(Check::Allowed("espace de travail".into()));
                }
                // Toujours confirmée, même dans un dossier autorisé.
                Check::Ask(Approval {
                    title: "Envoyer à la corbeille".into(),
                    detail: p.display().to_string(),
                    options: vec![Answer::Once],
                    dangerous: true,
                })
            }
            Action::Fetch { url } => {
                let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
                let known = self
                    .grants(session)
                    .iter()
                    .any(|g| matches!(g, Grant::Host(h) if *h == host));
                match policy.network {
                    Policy::Deny => Check::Denied("l'accès au web est désactivé".into()),
                    Policy::Allow => Check::Allowed("web autorisé".into()),
                    Policy::Ask if known => Check::Allowed("site autorisé".into()),
                    Policy::Ask => Check::Ask(Approval {
                        title: format!("Consulter le site {host}"),
                        detail: url.to_string(),
                        options: vec![Answer::Once, Answer::Session, Answer::Always],
                        dangerous: false,
                    }),
                }
            }
            Action::Shell {
                command,
                cwd,
                sandboxed,
            } => match policy.shell {
                Policy::Deny => Check::Denied("les commandes sont désactivées".into()),
                // Une commande est toujours confirmée : elle peut tout faire.
                _ => Check::Ask(Approval {
                    title: if *sandboxed {
                        "Exécuter une commande PowerShell (bac à sable)".into()
                    } else {
                        "Exécuter une commande PowerShell SANS bac à sable".into()
                    },
                    detail: format!("{}\n\ndans {}", command, cwd.display()),
                    options: vec![Answer::Once],
                    dangerous: !sandboxed,
                }),
            },
            Action::Mcp { server, tool, args } => {
                let known = self.grants(session).iter().any(
                    |g| matches!(g, Grant::McpTool { server: s, tool: t } if s == server && t == tool),
                );
                match policy.mcp {
                    Policy::Deny => Check::Denied("les outils MCP sont désactivés".into()),
                    Policy::Allow => Check::Allowed("outils MCP autorisés".into()),
                    Policy::Ask if known => Check::Allowed("outil MCP autorisé".into()),
                    Policy::Ask => Check::Ask(Approval {
                        title: format!("Utiliser l'outil « {tool} » du serveur MCP {server}"),
                        detail: args.clone(),
                        options: vec![Answer::Once, Answer::Session, Answer::Always],
                        dangerous: false,
                    }),
                }
            }
        })
    }

    /// Retient la réponse de l'utilisateur (sans effet pour `Once` et `Deny`).
    pub fn remember(&self, action: &Action, answer: Answer, session: &str) -> Result<()> {
        if !matches!(answer, Answer::Session | Answer::Always) {
            return Ok(());
        }
        let grant = match action {
            Action::Read(p) => Grant::Folder {
                path: folder_of(&normalize(p)?),
                write: false,
            },
            Action::Write { path, overwrite } => {
                let p = normalize(path)?;
                let folder = folder_of(&p);
                if *overwrite && self.folder_access(&p, session) == Some(true) {
                    Grant::Overwrite(folder)
                } else {
                    Grant::Folder {
                        path: folder,
                        write: true,
                    }
                }
            }
            Action::Fetch { url } => {
                Grant::Host(url.host_str().unwrap_or_default().to_ascii_lowercase())
            }
            Action::Mcp { server, tool, .. } => Grant::McpTool {
                server: server.clone(),
                tool: tool.clone(),
            },
            // Jamais retenu : confirmé à chaque fois.
            Action::Delete(_) | Action::Shell { .. } => return Ok(()),
        };
        match (answer, &grant) {
            (Answer::Always, Grant::Folder { path, write }) => {
                self.db
                    .add_grant("folder", &path.display().to_string(), *write)?
            }
            (Answer::Always, Grant::Host(h)) => self.db.add_grant("host", h, false)?,
            (Answer::Always, Grant::McpTool { server, tool }) => {
                self.db
                    .add_grant("mcp", &format!("{server}/{tool}"), false)?
            }
            _ => self
                .session
                .lock()
                .unwrap()
                .entry(session.to_owned())
                .or_default()
                .push(grant),
        }
        Ok(())
    }

    /// Dossier accordé depuis l'interface (sélecteur de dossier).
    pub fn grant_folder(&self, path: &Path, write: bool) -> Result<()> {
        let p = normalize(path)?;
        if let Some(why) = self.protection(&p, write) {
            anyhow::bail!(why);
        }
        self.db.add_grant("folder", &p.display().to_string(), write)
    }

    /// Dossiers accessibles dans cette conversation (pour informer le modèle).
    pub fn folders(&self, session: &str) -> Vec<(PathBuf, bool)> {
        let mut out = vec![(self.workspace.clone(), true)];
        for g in self.grants(session) {
            if let Grant::Folder { path, write } = g {
                out.push((path, write));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        engine: Engine,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        for d in ["ws", "docs", "secret", "sys"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        std::fs::write(root.join("docs").join("a.txt"), "x").unwrap();
        let engine = Engine::new(
            Database::open_in_memory().unwrap(),
            Permissions::default(),
            Protected {
                no_access: vec![root.join("secret")],
                read_only: vec![root.join("sys")],
            },
            root.join("ws"),
        );
        Fixture {
            _dir: dir,
            root,
            engine,
        }
    }

    fn allowed(c: Check) -> bool {
        matches!(c, Check::Allowed(_))
    }

    #[test]
    fn nothing_is_allowed_by_default_except_workspace() {
        let f = fixture();
        let e = &f.engine;
        assert!(matches!(
            e.check(&Action::Read(f.root.join("docs/a.txt")), "s")
                .unwrap(),
            Check::Ask(_)
        ));
        assert!(allowed(
            e.check(
                &Action::Write {
                    path: f.root.join("ws/out.md"),
                    overwrite: false
                },
                "s"
            )
            .unwrap()
        ));
        assert!(allowed(
            e.check(&Action::Delete(f.root.join("ws/out.md")), "s")
                .unwrap()
        ));
    }

    #[test]
    fn session_and_permanent_grants() {
        let f = fixture();
        let e = &f.engine;
        let read = Action::Read(f.root.join("docs/a.txt"));
        e.remember(&read, Answer::Session, "s1").unwrap();
        assert!(allowed(e.check(&read, "s1").unwrap()));
        assert!(
            matches!(e.check(&read, "s2").unwrap(), Check::Ask(_)),
            "propre à la conversation"
        );
        // Lecture accordée n'implique pas l'écriture.
        let write = Action::Write {
            path: f.root.join("docs/b.txt"),
            overwrite: false,
        };
        assert!(matches!(e.check(&write, "s1").unwrap(), Check::Ask(_)));
        e.remember(&write, Answer::Always, "s1").unwrap();
        assert!(allowed(e.check(&write, "s9").unwrap()), "permanent");
        // Écraser demande encore confirmation, puis peut être retenu pour la conversation.
        let over = Action::Write {
            path: f.root.join("docs/a.txt"),
            overwrite: true,
        };
        let Check::Ask(a) = e.check(&over, "s9").unwrap() else {
            panic!()
        };
        assert!(a.dangerous);
        e.remember(&over, Answer::Session, "s9").unwrap();
        assert!(allowed(e.check(&over, "s9").unwrap()));
        // Supprimer est toujours confirmé.
        e.remember(
            &Action::Delete(f.root.join("docs/a.txt")),
            Answer::Always,
            "s9",
        )
        .unwrap();
        assert!(matches!(
            e.check(&Action::Delete(f.root.join("docs/a.txt")), "s9")
                .unwrap(),
            Check::Ask(_)
        ));
    }

    #[test]
    fn traversal_and_protected_paths() {
        let f = fixture();
        let e = &f.engine;
        e.grant_folder(&f.root.join("docs"), true).unwrap();
        let escape = f.root.join("docs").join("..").join("secret").join("k.txt");
        assert!(matches!(
            e.check(&Action::Read(escape), "s").unwrap(),
            Check::Denied(_)
        ));
        assert!(e.grant_folder(&f.root.join("secret"), false).is_err());
        assert!(matches!(
            e.check(
                &Action::Write {
                    path: f.root.join("sys/x.dll"),
                    overwrite: false
                },
                "s"
            )
            .unwrap(),
            Check::Denied(_)
        ));
        assert!(matches!(
            e.check(&Action::Read(f.root.join("sys/x.dll")), "s")
                .unwrap(),
            Check::Ask(_)
        ));
    }

    #[test]
    fn network_shell_and_mcp_policies() {
        let f = fixture();
        let e = &f.engine;
        let fetch = Action::Fetch {
            url: "https://example.com/page?q=1".parse().unwrap(),
        };
        assert!(matches!(e.check(&fetch, "s").unwrap(), Check::Ask(_)));
        e.remember(&fetch, Answer::Always, "s").unwrap();
        let other_page = Action::Fetch {
            url: "https://EXAMPLE.com/autre".parse().unwrap(),
        };
        assert!(allowed(e.check(&other_page, "s").unwrap()));

        let shell = Action::Shell {
            command: "dir".into(),
            cwd: f.root.clone(),
            sandboxed: true,
        };
        e.remember(&shell, Answer::Always, "s").unwrap();
        assert!(
            matches!(e.check(&shell, "s").unwrap(), Check::Ask(_)),
            "jamais retenu"
        );

        let mut p = e.policy();
        p.network = Policy::Deny;
        p.shell = Policy::Deny;
        e.set_policy(p);
        assert!(matches!(
            e.check(&other_page, "s").unwrap(),
            Check::Denied(_)
        ));
        assert!(matches!(e.check(&shell, "s").unwrap(), Check::Denied(_)));

        let mcp = Action::Mcp {
            server: "gh".into(),
            tool: "search".into(),
            args: "{}".into(),
        };
        e.remember(&mcp, Answer::Session, "s").unwrap();
        assert!(allowed(e.check(&mcp, "s").unwrap()));
    }
}
