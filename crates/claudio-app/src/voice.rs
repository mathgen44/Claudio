//! Voix : installation des moteurs, dictée, mot d'éveil et lecture des réponses.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use claudio_core::config::TtsEngine;
use claudio_models::Progress;
use claudio_voice::components::{self, PIPER, WAKE_MODEL, WHISPER};
use claudio_voice::tts::{self, SystemVoice};
use claudio_voice::{Kind, text};
use serde::Serialize;

use crate::Claudio;

#[derive(Debug, Clone, Serialize)]
pub struct VoiceComponentView {
    pub id: String,
    pub label: String,
    pub description: String,
    pub kind: Kind,
    pub size: u64,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceStatus {
    pub components: Vec<VoiceComponentView>,
    /// Modèle de dictée utilisé (choisi ou conseillé pour la machine).
    pub stt_model: String,
    /// Éléments à installer avant de pouvoir dicter.
    pub dictation_missing: Vec<String>,
    /// Éléments à installer avant de pouvoir guetter le mot d'éveil.
    pub wake_missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transcript {
    pub text: String,
    /// Écoute du mot d'éveil : la demande qui suit le mot (vide s'il a été dit
    /// seul), `None` s'il n'a pas été prononcé.
    pub wake: Option<String>,
    pub seconds: f32,
}

/// Durées maximales des enregistrements envoyés à Whisper.
const MAX_DICTATION_SECS: f32 = 120.0;
const MAX_WAKE_SECS: f32 = 10.0;
/// Au-delà, la lecture à voix haute est tronquée.
const MAX_SPEECH_CHARS: usize = 4000;

impl Claudio {
    fn stt_model(&self) -> &'static str {
        self.config()
            .voice
            .stt_model
            .as_deref()
            .and_then(components::get)
            .filter(|c| c.kind == Kind::Stt)
            .map(|c| c.id)
            .unwrap_or_else(|| components::default_stt(self.hardware().profile().tier))
    }

    fn missing(&self, ids: &[&str]) -> Vec<String> {
        let root = self.paths.voice_dir();
        ids.iter()
            .filter(|id| components::get(id).is_some_and(|c| !c.is_installed(&root)))
            .map(|id| id.to_string())
            .collect()
    }

    pub fn voice_status(&self) -> VoiceStatus {
        let root = self.paths.voice_dir();
        let stt = self.stt_model();
        VoiceStatus {
            components: components::ALL
                .iter()
                .map(|c| VoiceComponentView {
                    id: c.id.into(),
                    label: c.label.into(),
                    description: c.description.into(),
                    kind: c.kind,
                    size: c.size(),
                    installed: c.is_installed(&root),
                })
                .collect(),
            stt_model: stt.into(),
            dictation_missing: self.missing(&[WHISPER, stt]),
            wake_missing: self.missing(&[WHISPER, WAKE_MODEL]),
        }
    }

    pub async fn install_voice(&self, id: &str, on_progress: impl FnMut(&Progress)) -> Result<()> {
        let c = components::get(id).with_context(|| format!("élément vocal inconnu : {id}"))?;
        let key = format!("voice:{id}");
        let cancel = self.begin_download(&key)?;
        let res =
            components::install(&self.http, &self.paths.voice_dir(), c, &cancel, on_progress).await;
        self.end_download(&key);
        res
    }

    pub async fn remove_voice(&self, id: &str) -> Result<()> {
        let c = components::get(id).with_context(|| format!("élément vocal inconnu : {id}"))?;
        // Un serveur Whisper peut tenir le modèle ou le moteur ouvert.
        self.whisper.stop_all().await;
        components::remove(&self.paths.voice_dir(), c)
    }

    pub async fn stop_voice(&self) {
        self.whisper.stop_all().await;
    }

    pub async fn system_voices(&self) -> Result<Vec<SystemVoice>> {
        tts::system_voices().await
    }

    /// Transcrit un enregistrement WAV ; `wake` : écoute du mot d'éveil (petit modèle).
    pub async fn transcribe(&self, wav: Vec<u8>, wake: bool) -> Result<Transcript> {
        let seconds = claudio_voice::wav_duration(&wav)?;
        let limit = if wake {
            MAX_WAKE_SECS
        } else {
            MAX_DICTATION_SECS
        };
        if seconds > limit {
            bail!("enregistrement trop long ({seconds:.0} s, {limit:.0} s au plus)");
        }
        let cfg = self.config().voice;
        let model_id = if wake { WAKE_MODEL } else { self.stt_model() };
        let missing = self.missing(&[WHISPER, model_id]);
        if !missing.is_empty() {
            bail!(
                "la reconnaissance vocale n'est pas installée (manque : {})",
                missing.join(", ")
            );
        }
        let root = self.paths.voice_dir();
        let exe = components::find_exe(
            &components::get(WHISPER).expect("moteur whisper").dir(&root),
            "whisper-server.exe",
        )?;
        let model = components::get(model_id)
            .expect("modèle vocal")
            .main_file(&root);
        let threads = self.hardware().profile().threads;
        // Le mot d'éveil est donné en indice pour que Whisper l'orthographie bien.
        let text = self
            .whisper
            .transcribe(
                &self.http,
                &claudio_voice::whisper::Model {
                    exe: &exe,
                    file: &model,
                    threads,
                },
                wav,
                &cfg.language,
                Some(cfg.wake_phrase.as_str()),
            )
            .await?;
        let wake = if wake {
            text::wake(&text, &cfg.wake_phrase)
        } else {
            None
        };
        Ok(Transcript {
            text,
            wake,
            seconds,
        })
    }

    /// Lit un texte (Markdown accepté) et renvoie l'audio en WAV.
    pub async fn synthesize(&self, markdown: &str) -> Result<Vec<u8>> {
        let mut speech = text::speakable(markdown);
        if speech.chars().count() > MAX_SPEECH_CHARS {
            speech = speech.chars().take(MAX_SPEECH_CHARS).collect();
            speech.push_str("… La suite est à l'écran.");
        }
        if speech.trim().is_empty() {
            bail!("rien à lire");
        }
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = self.paths.voice_dir().join("tmp");
        std::fs::create_dir_all(&dir)?;
        let out = dir.join(format!(
            "{}-{}.wav",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let cfg = self.config().voice;
        let root = self.paths.voice_dir();
        let piper = components::get(PIPER).expect("moteur Piper");
        let voice = components::get(&cfg.piper_voice).filter(|c| c.kind == Kind::Voice);
        let res = match (cfg.tts_engine, voice) {
            (TtsEngine::Piper, Some(v)) if piper.is_installed(&root) && v.is_installed(&root) => {
                let exe = components::find_exe(&piper.dir(&root), "piper.exe")?;
                match tts::speak_piper(&exe, &v.main_file(&root), &speech, cfg.rate, &out).await {
                    Ok(()) => Ok(()),
                    // Mieux vaut la voix Windows que le silence.
                    Err(e) => {
                        tracing::warn!("Piper : {e:#} ; voix Windows utilisée");
                        let sapi = cfg.system_voice.as_deref();
                        tts::speak_sapi(&speech, sapi, &cfg.language, cfg.rate, &out).await
                    }
                }
            }
            (engine, _) => {
                if engine == TtsEngine::Piper {
                    tracing::warn!("Piper n'est pas installé : voix Windows utilisée");
                }
                tts::speak_sapi(
                    &speech,
                    cfg.system_voice.as_deref(),
                    &cfg.language,
                    cfg.rate,
                    &out,
                )
                .await
            }
        };
        let wav = res.and_then(|_| std::fs::read(&out).context("lecture de l'audio produit"));
        std::fs::remove_file(&out).ok();
        wav
    }
}
