//! Le impostazioni: il modello dell'agente, quello della trascrizione, la chiave di Gemini e il
//! microfono. Stanno in `~/.config/raccolta-requisiti/impostazioni.json`; la chiave a parte, in
//! `gemini.key`, leggibile solo dall'utente. `RR_CONFIG_DIR` sposta la cartella (nei test).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_AGENT_MODEL: &str = "gpt-6.1-terra";
pub const DEFAULT_TRANSCRIBE_MODEL: &str = "gemini-3.5-transcribe";
/// I livelli di ragionamento che si possono scegliere per l'agente, dal più rapido.
pub const EFFORTS: [&str; 3] = ["low", "medium", "high"];
const FILE: &str = "impostazioni.json";
const KEY_FILE: &str = "gemini.key";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub agent_model: String,
    pub agent_effort: String,
    /// Il servizio prioritario di Codex: risposte più rapide, consumo 1,5×.
    pub agent_fast: bool,
    pub transcribe_model: String,
    pub transcribe_lang: String,
    /// La sorgente PipeWire da cui registrare; vuota = il microfono predefinito del sistema.
    pub audio_source: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            agent_model: DEFAULT_AGENT_MODEL.into(),
            agent_effort: "low".into(),
            agent_fast: true,
            transcribe_model: DEFAULT_TRANSCRIBE_MODEL.into(),
            transcribe_lang: std::env::var("RR_LANG").unwrap_or_else(|_| "it-IT".into()),
            audio_source: std::env::var("RR_AUDIO_SOURCE").ok().filter(|s| s != "default").unwrap_or_default(),
        }
    }
}

/// Come chiamare l'agente: modello, ragionamento e servizio prioritario.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentConfig {
    pub bin: String,
    pub model: String,
    pub effort: String,
    pub fast: bool,
}

impl Settings {
    /// Le impostazioni salvate; quelle mancanti o rovinate prendono il valore predefinito.
    pub fn load(dir: &Path) -> Settings {
        let s: Settings = std::fs::read_to_string(dir.join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        s.cleaned()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, dir.join(FILE))
    }

    /// Niente campi vuoti o fuori elenco: al loro posto il valore predefinito.
    pub fn cleaned(mut self) -> Settings {
        let d = Settings::default();
        let fix = |v: &mut String, def: String| {
            *v = v.trim().to_string();
            if v.is_empty() {
                *v = def;
            }
        };
        fix(&mut self.agent_model, d.agent_model);
        fix(&mut self.transcribe_model, d.transcribe_model);
        fix(&mut self.transcribe_lang, d.transcribe_lang);
        if !EFFORTS.contains(&self.agent_effort.as_str()) {
            self.agent_effort = d.agent_effort;
        }
        self.audio_source = self.audio_source.trim().to_string();
        self
    }

    /// L'agente per i giri sulla lavagna; l'esportazione usa lo stesso modello con più ragionamento.
    pub fn agent(&self) -> AgentConfig {
        AgentConfig { bin: crate::agent::codex_bin(), model: self.agent_model.clone(), effort: self.agent_effort.clone(), fast: self.agent_fast }
    }

    /// Il nome da passare a ffmpeg: `default` è il microfono predefinito di PipeWire.
    pub fn source_for_ffmpeg(&self) -> String {
        if self.audio_source.is_empty() { "default".into() } else { self.audio_source.clone() }
    }
}

/// La cartella delle impostazioni: `RR_CONFIG_DIR`, altrimenti `~/.config/raccolta-requisiti`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("RR_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("raccolta-requisiti"))
}

// ---- la chiave di Gemini ----

/// Da dove viene la chiave in uso.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Salvata dalle impostazioni.
    Saved,
    /// Dalla variabile `GEMINI_API_KEY`.
    Env,
    None,
}

fn clean_key(k: &str) -> Option<String> {
    let k = k.trim();
    (!k.is_empty()).then(|| k.to_string())
}

pub fn saved_key(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(KEY_FILE)).ok().and_then(|k| clean_key(&k))
}

/// La chiave da usare: quella salvata nelle impostazioni, altrimenti `GEMINI_API_KEY`.
pub fn active_key(dir: Option<&Path>) -> (Option<String>, KeySource) {
    if let Some(k) = dir.and_then(saved_key) {
        return (Some(k), KeySource::Saved);
    }
    match std::env::var("GEMINI_API_KEY").ok().and_then(|k| clean_key(&k)) {
        Some(k) => (Some(k), KeySource::Env),
        None => (None, KeySource::None),
    }
}

/// Una chiave che si può mettere nell'intestazione HTTP: niente spazi, virgolette o a capo.
pub fn key_problem(k: &str) -> Option<&'static str> {
    if k.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\') {
        Some("la chiave contiene spazi o caratteri non validi")
    } else if k.len() < 20 {
        Some("la chiave sembra troppo corta")
    } else {
        None
    }
}

/// Salva la chiave leggibile solo dall'utente (0600), sostituendo quella di prima.
pub fn save_key(dir: &Path, key: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    std::fs::create_dir_all(dir)?;
    let path = dir.join(KEY_FILE);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    // Se il file c'era già con altri permessi, `mode` non li cambia: si stringono qui.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(key.trim().as_bytes())?;
    f.write_all(b"\n")
}

/// `AQ.abcdefgh…wxyz` → `…wxyz`: abbastanza per riconoscerla, non per usarla.
pub fn mask(key: &str) -> String {
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("•••• {tail}")
}

// ---- i microfoni ----

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSource {
    /// Il nome PipeWire, quello che si passa a ffmpeg.
    pub name: String,
    pub description: String,
    pub muted: bool,
}

/// Le sorgenti da cui si può registrare, senza i «monitor» delle uscite audio.
pub fn audio_sources() -> Result<Vec<AudioSource>, String> {
    let out = Command::new("pactl").args(["-f", "json", "list", "sources"]).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("non riesco a chiedere i microfoni a PipeWire: {e}"))?;
    if !out.status.success() {
        return Err("PipeWire non ha risposto (pactl)".into());
    }
    parse_sources(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_sources(json: &str) -> Result<Vec<AudioSource>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("elenco dei microfoni illeggibile: {e}"))?;
    let mut out = Vec::new();
    for s in v.as_array().into_iter().flatten() {
        let name = s["name"].as_str().unwrap_or("").to_string();
        let monitor = !s["monitor_source"].as_str().unwrap_or("").is_empty() || name.ends_with(".monitor") || s["properties"]["device.class"] == "monitor";
        if name.is_empty() || monitor {
            continue;
        }
        let description = s["description"].as_str().filter(|d| !d.trim().is_empty()).unwrap_or(&name).to_string();
        out.push(AudioSource { name, description, muted: s["mute"].as_bool().unwrap_or(false) });
    }
    Ok(out)
}

/// Il microfono predefinito di PipeWire, se lo dice.
pub fn default_source() -> Option<String> {
    let out = Command::new("pactl").arg("get-default-source").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
}

// ---- i modelli ----

/// I modelli che Codex mostra nel suo elenco (`slug`, nome da mostrare), dalla cache che la CLI
/// tiene in `~/.codex/models_cache.json`. Si legge solo quel file, mai le credenziali.
pub fn agent_models() -> Vec<(String, String)> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")));
    home.and_then(|h| std::fs::read_to_string(h.join("models_cache.json")).ok()).map(|t| parse_models(&t)).unwrap_or_default()
}

pub fn parse_models(json: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let mut models: Vec<(i64, String, String)> = v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            let slug = m["slug"].as_str()?.to_string();
            let name = m["display_name"].as_str().unwrap_or(&slug).to_string();
            Some((m["priority"].as_i64().unwrap_or(i64::MAX), slug, name))
        })
        .collect();
    models.sort_by_key(|m| m.0);
    models.into_iter().map(|(_, slug, name)| (slug, name)).collect()
}

/// Come è collegato Codex (`codex login status`), in una riga.
pub fn codex_login(bin: &str) -> String {
    match Command::new(bin).args(["login", "status"]).stdin(Stdio::null()).output() {
        Ok(out) => {
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
            match (out.status.success(), line.as_str()) {
                (true, l) if l.contains("ChatGPT") => "Collegato con l'abbonamento ChatGPT".into(),
                (true, l) if l.contains("API key") => "Collegato con una chiave API OpenAI".into(),
                (true, "") => "Collegato".into(),
                (true, l) => l.to_string(),
                (false, _) => "Non collegato: esegui «codex login» in un terminale".into(),
            }
        }
        Err(_) => "Codex non è installato o non si trova".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rr-settings-{name}-{}", std::process::id()))
    }

    #[test]
    fn settings_survive_a_save_and_a_broken_file_falls_back_to_defaults() {
        let dir = temp("roundtrip");
        let s = Settings { agent_model: "gpt-6-luna".into(), agent_effort: "high".into(), agent_fast: false, audio_source: "alsa_input.usb-Rode".into(), ..Settings::default() };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        std::fs::write(dir.join(FILE), "{ rotto").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_wrong_fields_take_the_defaults() {
        let dir = temp("partial");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), r#"{"agent_model":"  ","agent_effort":"turbo","transcribe_lang":"en-US"}"#).unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.agent_model, DEFAULT_AGENT_MODEL);
        assert_eq!(s.agent_effort, "low");
        assert_eq!(s.transcribe_lang, "en-US");
        assert_eq!(s.transcribe_model, DEFAULT_TRANSCRIBE_MODEL);
        assert_eq!(s.source_for_ffmpeg(), "default");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_saved_key_is_private_and_wins_over_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("key");
        assert_eq!(saved_key(&dir), None);
        save_key(&dir, "  AQ.chiave-di-prova-abbastanza-lunga \n").unwrap();
        assert_eq!(saved_key(&dir).as_deref(), Some("AQ.chiave-di-prova-abbastanza-lunga"));
        let mode = std::fs::metadata(dir.join(KEY_FILE)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "solo l'utente la legge");
        assert_eq!(active_key(Some(&dir)).1, KeySource::Saved);
        save_key(&dir, "AQ.seconda-chiave-che-sostituisce").unwrap();
        assert_eq!(saved_key(&dir).as_deref(), Some("AQ.seconda-chiave-che-sostituisce"), "la nuova sostituisce la vecchia");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_key_with_spaces_or_quotes_is_refused_and_a_masked_key_shows_only_its_end() {
        assert!(key_problem("AQ.abc def-ghijklmnopqrstu").is_some());
        assert!(key_problem("AQ.abc\"defghijklmnopqrstu").is_some());
        assert!(key_problem("corta").is_some());
        assert!(key_problem("AQ.esempio-chiave-valida_123").is_none());
        assert_eq!(mask("AQ.segretissima1234"), "•••• 1234");
    }

    #[test]
    fn monitors_are_not_microphones() {
        let json = r#"[
          {"name":"alsa_output.pci.analog-stereo.monitor","description":"Monitor of Built-in","monitor_source":"alsa_output.pci.analog-stereo","mute":false,"properties":{"device.class":"monitor"}},
          {"name":"alsa_input.pci.analog-stereo","description":"Built-in Audio","monitor_source":"","mute":false,"properties":{"device.class":"sound"}},
          {"name":"alsa_input.usb-RODE_NT-USB","description":"RØDE NT-USB","monitor_source":"","mute":true,"properties":{}}
        ]"#;
        let s = parse_sources(json).unwrap();
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["alsa_input.pci.analog-stereo", "alsa_input.usb-RODE_NT-USB"]);
        assert!(s[1].muted);
        assert!(parse_sources("non json").is_err());
    }

    #[test]
    fn the_agent_models_are_the_listed_ones_in_codex_order() {
        let json = r#"{"models":[
          {"slug":"gpt-6.1-terra","display_name":"GPT-6.1 Terra","visibility":"list","priority":3},
          {"slug":"segreto","display_name":"Nascosto","visibility":"hide","priority":1},
          {"slug":"gpt-6.1-sol","display_name":"GPT-6.1-Sol","visibility":"list","priority":1}
        ]}"#;
        let m = parse_models(json);
        assert_eq!(m, [("gpt-6.1-sol".to_string(), "GPT-6.1-Sol".to_string()), ("gpt-6.1-terra".to_string(), "GPT-6.1 Terra".to_string())]);
        assert!(parse_models("{}").is_empty());
    }
}
