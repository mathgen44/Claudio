//! Exécution de commandes PowerShell, en bac à sable ou non.
//!
//! Bac à sable : le processus reçoit un jeton d'intégrité **basse**. Windows lui
//! interdit alors d'écrire dans les dossiers de l'utilisateur, du système et le
//! registre ; il ne peut écrire que dans l'espace de travail de Claudio (étiqueté
//! « intégrité basse »). La lecture et le réseau restent possibles : c'est une
//! barrière contre les modifications, pas un isolement complet.
//!
//! Dans les deux cas : job Windows (arrêt de tous les sous-processus à la fin,
//! mémoire et nombre de processus plafonnés), délai maximal, sortie tronquée,
//! variables d'environnement contenant des secrets retirées, et seuls les tuyaux
//! de sortie sont transmis au processus (aucun autre handle hérité).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct ShellRequest {
    pub script: String,
    pub cwd: PathBuf,
    pub sandboxed: bool,
    pub timeout: Duration,
    /// Dossier temporaire du processus (doit être inscriptible en intégrité basse).
    pub temp_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellOutput {
    pub exit_code: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
}

const MAX_OUTPUT: usize = 64 * 1024;
const JOB_MEMORY: usize = 2 * 1024 * 1024 * 1024;
const JOB_PROCESSES: u32 = 32;

/// Variables jamais transmises : elles contiennent souvent des secrets.
fn is_secret_var(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|w| n.contains(w))
}

fn environment(temp: &Path) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| !is_secret_var(k) && !k.starts_with('='))
        .filter(|(k, _)| !k.eq_ignore_ascii_case("TEMP") && !k.eq_ignore_ascii_case("TMP"))
        .collect();
    let t = temp.display().to_string();
    vars.push(("TEMP".into(), t.clone()));
    vars.push(("TMP".into(), t));
    vars.sort_by_key(|(k, _)| k.to_ascii_uppercase());
    vars
}

/// Ligne de commande PowerShell ; le script est encodé pour éviter tout problème de guillemets.
fn command_line(script: &str) -> String {
    let full = format!(
        "$ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; {script}"
    );
    let utf16: Vec<u8> = full.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {encoded}"
    )
}

fn truncate(bytes: Vec<u8>, truncated: &mut bool) -> String {
    let mut s = String::from_utf8_lossy(&bytes).into_owned();
    if s.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n… (sortie tronquée)");
        *truncated = true;
    }
    s
}

/// Étiquette un dossier en intégrité basse pour que le bac à sable puisse y écrire.
pub fn label_low_integrity(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let marker = dir.join(".claudio-low-integrity");
    if marker.exists() {
        return Ok(());
    }
    let status = std::process::Command::new("icacls")
        .arg(dir)
        .args(["/setintegritylevel", "(OI)(CI)low"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("lancement d'icacls")?;
    if !status.success() {
        bail!(
            "impossible d'étiqueter {} en intégrité basse",
            dir.display()
        );
    }
    std::fs::write(marker, b"")?;
    Ok(())
}

/// Exécute la commande (bloquant : à appeler depuis `spawn_blocking`).
pub fn run(req: &ShellRequest) -> Result<ShellOutput> {
    if !req.cwd.is_dir() {
        bail!("dossier de travail introuvable : {}", req.cwd.display());
    }
    std::fs::create_dir_all(&req.temp_dir)?;
    imp::run(req)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::Read;
    use std::os::windows::io::FromRawHandle;

    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_TIMEOUT,
    };
    use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows::Win32::Security::{
        DuplicateTokenEx, PSID, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES, SecurityImpersonation,
        SetTokenInformation, TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY,
        TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel, TokenPrimary,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        UpdateProcThreadAttribute, WaitForSingleObject,
    };
    use windows::core::{PCWSTR, PWSTR, w};

    use super::*;

    /// Attribut « niveau d'intégrité » d'un groupe de jeton (winnt.h).
    const SE_GROUP_INTEGRITY: u32 = 0x20;

    /// Ferme le handle à la sortie de portée.
    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Copie du jeton du processus courant, en intégrité basse.
    unsafe fn low_integrity_token() -> Result<Owned> {
        unsafe {
            let mut own = HANDLE::default();
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
                &mut own,
            )?;
            let own = Owned(own);
            let mut dup = HANDLE::default();
            DuplicateTokenEx(
                own.0,
                TOKEN_ACCESS_MASK(0),
                None,
                SecurityImpersonation,
                TokenPrimary,
                &mut dup,
            )?;
            let dup = Owned(dup);
            let mut sid = PSID::default();
            // S-1-16-4096 : niveau d'intégrité « bas ».
            ConvertStringSidToSidW(w!("S-1-16-4096"), &mut sid)?;
            let label = TOKEN_MANDATORY_LABEL {
                Label: SID_AND_ATTRIBUTES {
                    Sid: sid,
                    Attributes: SE_GROUP_INTEGRITY,
                },
            };
            let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>()
                + windows::Win32::Security::GetLengthSid(sid) as usize;
            let res = SetTokenInformation(
                dup.0,
                TokenIntegrityLevel,
                &label as *const _ as *const c_void,
                size as u32,
            );
            let _ = windows::Win32::Foundation::LocalFree(Some(
                windows::Win32::Foundation::HLOCAL(sid.0),
            ));
            res?;
            Ok(dup)
        }
    }

    unsafe fn job() -> Result<Owned> {
        unsafe {
            let job = Owned(CreateJobObjectW(None, PCWSTR::null())?);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_JOB_MEMORY;
            info.BasicLimitInformation.ActiveProcessLimit = JOB_PROCESSES;
            info.JobMemoryLimit = JOB_MEMORY;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            Ok(job)
        }
    }

    /// Tuyau dont seule l'extrémité d'écriture est héritable.
    unsafe fn pipe() -> Result<(Owned, Owned)> {
        unsafe {
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: true.into(),
            };
            let (mut r, mut w) = (HANDLE::default(), HANDLE::default());
            CreatePipe(&mut r, &mut w, Some(&sa), 0)?;
            SetHandleInformation(r, HANDLE_FLAG_INHERIT.0, Default::default())?;
            Ok((Owned(r), Owned(w)))
        }
    }

    fn reader(h: Owned) -> std::thread::JoinHandle<Vec<u8>> {
        let raw = h.0.0;
        std::mem::forget(h);
        // SAFETY : le handle nous appartient désormais, `File` le fermera.
        let mut f = unsafe { File::from_raw_handle(raw as _) };
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            while let Ok(n) = f.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                // On continue à lire pour ne pas bloquer le processus, sans tout garder.
                if buf.len() < MAX_OUTPUT * 2 {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
            buf
        })
    }

    pub fn run(req: &ShellRequest) -> Result<ShellOutput> {
        unsafe {
            let token = if req.sandboxed {
                Some(low_integrity_token().context("création du jeton du bac à sable")?)
            } else {
                None
            };
            let job = job().context("création du job")?;
            let (out_r, out_w) = pipe()?;
            let (err_r, err_w) = pipe()?;

            // Seuls ces deux handles sont transmis au processus.
            let inherit = [out_w.0, err_w.0];
            let mut size = 0usize;
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
            let mut attr_buf = vec![0u8; size];
            let attrs = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut c_void);
            InitializeProcThreadAttributeList(Some(attrs), 1, None, &mut size)?;
            UpdateProcThreadAttribute(
                attrs,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(inherit.as_ptr() as *const c_void),
                std::mem::size_of_val(&inherit),
                None,
                None,
            )?;

            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdOutput = out_w.0;
            si.StartupInfo.hStdError = err_w.0;
            si.StartupInfo.hStdInput = HANDLE::default();
            si.lpAttributeList = attrs;

            let mut env: Vec<u16> = Vec::new();
            for (k, v) in environment(&req.temp_dir) {
                env.extend(format!("{k}={v}").encode_utf16());
                env.push(0);
            }
            env.push(0);
            let mut cmd = wide(&command_line(&req.script));
            let cwd = wide(&req.cwd.display().to_string());
            let flags = CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT;
            let mut pi = PROCESS_INFORMATION::default();
            let created = match &token {
                Some(t) => CreateProcessAsUserW(
                    Some(t.0),
                    PCWSTR::null(),
                    Some(PWSTR(cmd.as_mut_ptr())),
                    None,
                    None,
                    true,
                    flags,
                    Some(env.as_ptr() as *const c_void),
                    PCWSTR(cwd.as_ptr()),
                    &si.StartupInfo,
                    &mut pi,
                ),
                None => CreateProcessW(
                    PCWSTR::null(),
                    Some(PWSTR(cmd.as_mut_ptr())),
                    None,
                    None,
                    true,
                    flags,
                    Some(env.as_ptr() as *const c_void),
                    PCWSTR(cwd.as_ptr()),
                    &si.StartupInfo,
                    &mut pi,
                ),
            };
            DeleteProcThreadAttributeList(attrs);
            created.context("lancement de PowerShell")?;
            let process = Owned(pi.hProcess);
            let thread = Owned(pi.hThread);
            // Nos copies des extrémités d'écriture : fermées pour détecter la fin de sortie.
            drop(out_w);
            drop(err_w);
            if let Err(e) = AssignProcessToJobObject(job.0, process.0) {
                let _ = TerminateJobObject(job.0, 1);
                return Err(e).context("rattachement au job");
            }
            ResumeThread(thread.0);
            drop(thread);

            let out_t = reader(out_r);
            let err_t = reader(err_r);
            let ms = req.timeout.as_millis().min(u32::MAX as u128) as u32;
            let timed_out = WaitForSingleObject(process.0, ms) == WAIT_TIMEOUT;
            if timed_out {
                let _ = TerminateJobObject(job.0, 1);
            }
            let mut code = 0u32;
            let exit_code =
                (!timed_out && GetExitCodeProcess(process.0, &mut code).is_ok()).then_some(code);
            // Fermer le job tue les sous-processus restants, ce qui libère les tuyaux.
            drop(job);
            let mut truncated = false;
            let stdout = truncate(out_t.join().unwrap_or_default(), &mut truncated);
            let stderr = truncate(err_t.join().unwrap_or_default(), &mut truncated);
            Ok(ShellOutput {
                exit_code,
                stdout,
                stderr,
                timed_out,
                truncated,
            })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn run(_: &ShellRequest) -> Result<ShellOutput> {
        bail!("les commandes ne sont disponibles que sous Windows")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn req(dir: &Path, script: &str, sandboxed: bool) -> ShellRequest {
        ShellRequest {
            script: script.into(),
            cwd: dir.to_path_buf(),
            sandboxed,
            timeout: Duration::from_secs(60),
            temp_dir: dir.join(".tmp"),
        }
    }

    #[test]
    fn hides_secret_variables() {
        assert!(is_secret_var("OPENAI_API_KEY"));
        assert!(is_secret_var("GITHUB_TOKEN"));
        assert!(!is_secret_var("PATH"));
    }

    #[test]
    fn runs_and_captures_output() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(&req(
            dir.path(),
            "Write-Output 'héllo'; Write-Error 'oups'; exit 3",
            false,
        ))
        .unwrap();
        assert_eq!(out.stdout.trim(), "héllo");
        assert!(out.stderr.contains("oups"));
        assert_eq!(out.exit_code, Some(3));
    }

    #[test]
    fn sandbox_can_only_write_in_low_integrity_folder() {
        let outside = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        label_low_integrity(ws.path()).unwrap();
        let target = outside.path().join("pirate.txt");
        let script = format!(
            "Set-Content -LiteralPath '{}' 'x' -ErrorAction Stop; 'ecrit'",
            target.display()
        );
        let out = run(&req(ws.path(), &script, true)).unwrap();
        assert!(
            !target.exists(),
            "le bac à sable a écrit hors de l'espace de travail : {out:?}"
        );
        let inside = ws.path().join("ok.txt");
        let script = format!(
            "Set-Content -LiteralPath '{}' 'x' -ErrorAction Stop; 'ecrit'",
            inside.display()
        );
        let out = run(&req(ws.path(), &script, true)).unwrap();
        assert!(
            inside.exists(),
            "écriture dans l'espace de travail refusée : {out:?}"
        );
    }

    #[test]
    fn timeout_kills_the_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = req(dir.path(), "Start-Sleep -Seconds 30", false);
        r.timeout = Duration::from_secs(2);
        let started = std::time::Instant::now();
        let out = run(&r).unwrap();
        assert!(out.timed_out);
        assert!(started.elapsed() < Duration::from_secs(15));
    }
}
