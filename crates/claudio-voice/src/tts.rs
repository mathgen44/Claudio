//! Synthèse vocale : voix Windows (SAPI, toujours disponibles) ou Piper.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use claudio_llm::process;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct SystemVoice {
    pub name: String,
    /// Langue, ex. `fr-FR`.
    pub culture: String,
}

const LIST_VOICES: &str = "Add-Type -AssemblyName System.Speech; \
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
$s.GetInstalledVoices() | Where-Object Enabled | ForEach-Object { $_.VoiceInfo.Name + '|' + $_.VoiceInfo.Culture.Name }";

// Le texte arrive par l'entrée standard et la voix par une variable d'environnement :
// rien de ce qui est lu à voix haute n'est interprété par PowerShell.
const SPEAK: &str = "[Console]::InputEncoding = [Text.Encoding]::UTF8; \
Add-Type -AssemblyName System.Speech; \
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
if ($env:CLAUDIO_TTS_VOICE) { $s.SelectVoice($env:CLAUDIO_TTS_VOICE) } \
elseif ($v = $s.GetInstalledVoices() | Where-Object { $_.Enabled -and $_.VoiceInfo.Culture.Name -like ($env:CLAUDIO_TTS_LANG + '*') } | Select-Object -First 1) { $s.SelectVoice($v.VoiceInfo.Name) } \
$s.Rate = [int]$env:CLAUDIO_TTS_RATE; \
$s.SetOutputToWaveFile($env:CLAUDIO_TTS_OUT); \
$s.Speak([Console]::In.ReadToEnd()); $s.Dispose()";

fn powershell(script: &str) -> Command {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process::hide_window(&mut cmd);
    cmd
}

pub async fn system_voices() -> Result<Vec<SystemVoice>> {
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        powershell(LIST_VOICES).stdin(Stdio::null()).output(),
    )
    .await
    .context("liste des voix : délai dépassé")??;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (name, culture) = l.trim().split_once('|')?;
            Some(SystemVoice {
                name: name.to_owned(),
                culture: culture.to_owned(),
            })
        })
        .collect())
}

async fn feed(mut child: tokio::process::Child, text: &str, limit: Duration) -> Result<()> {
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
    }
    let out = tokio::time::timeout(limit, child.wait_with_output())
        .await
        .context("synthèse vocale : délai dépassé")??;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.lines().rev().find(|l| !l.trim().is_empty());
        bail!(
            "synthèse vocale en échec ({}){}",
            out.status,
            detail
                .map(|d| format!(" : {}", d.trim()))
                .unwrap_or_default()
        );
    }
    Ok(())
}

/// Vitesse de -10 (lent) à 10 (rapide), 0 par défaut.
pub async fn speak_sapi(
    text: &str,
    voice: Option<&str>,
    language: &str,
    rate: i32,
    out: &Path,
) -> Result<()> {
    let child = powershell(SPEAK)
        .env("CLAUDIO_TTS_VOICE", voice.unwrap_or(""))
        .env("CLAUDIO_TTS_LANG", language)
        .env("CLAUDIO_TTS_RATE", rate.clamp(-10, 10).to_string())
        .env("CLAUDIO_TTS_OUT", out)
        .spawn()
        .context("lancement de PowerShell")?;
    feed(child, text, Duration::from_secs(120)).await
}

pub async fn speak_piper(
    exe: &Path,
    voice: &Path,
    text: &str,
    rate: i32,
    out: &Path,
) -> Result<()> {
    // Piper règle la durée des sons : plus grand = plus lent.
    let length_scale = 1.0 - rate.clamp(-10, 10) as f32 * 0.04;
    let dir = exe.parent().unwrap_or(exe);
    let mut cmd = Command::new(exe);
    cmd.arg("--model")
        .arg(crate::relative_to(dir, voice))
        .arg("--output_file")
        .arg(crate::relative_to(dir, out))
        .args(["--espeak_data", "espeak-ng-data"])
        .args(["--length_scale", &format!("{length_scale:.2}")])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process::hide_window(&mut cmd);
    let child = cmd.spawn().context("lancement de Piper")?;
    process::attach(&child);
    feed(child, text, Duration::from_secs(120)).await
}
