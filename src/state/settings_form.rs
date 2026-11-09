//! Il pannello delle impostazioni: una bozza delle scelte che diventa effettiva con «Salva».
//! «Annulla» o la chiusura la buttano.

use std::time::{Duration, Instant};

use rune::{Color, Live, TextEdit};

use super::State;
use crate::meter::LevelMeter;
use crate::settings::{self, AudioSource, KeySource, Settings};
use crate::transcribe::Check;

/// La barra del livello del microfono nel pannello (larghezza utile del pannello).
pub const LEVEL_W: f32 = 528.0;
pub const LEVEL_H: f32 = 10.0;
/// Mentre si trascina il cursore del volume, al sistema arriva al massimo un valore ogni tanto.
const VOLUME_EVERY: Duration = Duration::from_millis(80);

/// Il microfono nel pannello: il suo volume d'ingresso e il livello dal vivo.
pub struct MicPanel {
    /// Il volume d'ingresso del microfono scelto, in percento (se PipeWire lo dice).
    pub volume: Option<u32>,
    /// I volumi com'erano prima di toccarli, per riportarli indietro con «Annulla».
    originals: Vec<(String, u32)>,
    pending: Option<(String, u32)>,
    last_apply: Option<Instant>,
    pub level_rect: Live<[f32; 4]>,
    pub level_color: Live<Color>,
    meter: Option<LevelMeter>,
    pub meter_error: Option<String>,
    pub error: Option<String>,
    pactl: String,
    ffmpeg: String,
}

impl MicPanel {
    fn new(pactl: &str, ffmpeg: &str) -> MicPanel {
        MicPanel {
            volume: None,
            originals: Vec::new(),
            pending: None,
            last_apply: None,
            level_rect: Live::new([0.0, 0.0, 0.0, LEVEL_H]),
            level_color: Live::new(crate::meter::zone_color(0.0)),
            meter: None,
            meter_error: None,
            error: None,
            pactl: pactl.into(),
            ffmpeg: ffmpeg.into(),
        }
    }

    /// Legge il volume di `source` e comincia ad ascoltarla (smette di ascoltare la precedente).
    fn listen(&mut self, source: &str) {
        self.meter = None;
        self.level_rect.set([0.0, 0.0, 0.0, LEVEL_H]);
        self.volume = settings::source_volume(&self.pactl, source);
        let input = if source.is_empty() { "default" } else { source };
        match LevelMeter::start(&self.ffmpeg, input, LEVEL_W, LEVEL_H, self.level_rect.clone(), self.level_color.clone()) {
            Ok(m) => {
                self.meter = Some(m);
                self.meter_error = None;
            }
            Err(e) => self.meter_error = Some(format!("non riesco ad ascoltare il microfono: {e}")),
        }
    }

    /// Cambia il volume d'ingresso di `source`: si sente subito. Mentre si trascina arriva al
    /// sistema un valore ogni `VOLUME_EVERY`; `now` lo manda comunque (rilascio, pulsanti).
    fn set_volume(&mut self, source: &str, pct: u32, now: bool) {
        let pct = pct.min(settings::VOLUME_MAX);
        if let Some(before) = self.volume
            && !self.originals.iter().any(|(s, _)| s == source)
        {
            self.originals.push((source.to_string(), before));
        }
        self.volume = Some(pct);
        self.pending = Some((source.to_string(), pct));
        if now || self.last_apply.is_none_or(|t| t.elapsed() >= VOLUME_EVERY) {
            self.flush(true);
        }
    }

    /// Manda al sistema il volume rimasto in attesa, se è il momento (o subito con `force`).
    fn flush(&mut self, force: bool) {
        if !force && self.last_apply.is_some_and(|t| t.elapsed() < VOLUME_EVERY) {
            return;
        }
        if let Some((source, pct)) = self.pending.take() {
            self.last_apply = Some(Instant::now());
            self.error = settings::set_source_volume(&self.pactl, &source, pct).err();
        }
    }

    /// «Annulla»: i microfoni toccati tornano al volume di prima.
    fn restore(&mut self) {
        self.pending = None;
        for (source, pct) in self.originals.drain(..) {
            let _ = settings::set_source_volume(&self.pactl, &source, pct);
        }
    }

    /// «Salva»: il volume scelto resta (lo ricorda PipeWire).
    fn keep(&mut self) {
        self.flush(true);
        self.originals.clear();
    }
}

/// Il menu a tendina aperto nel pannello.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    Model,
    Source,
}

/// La prova della chiave di Gemini.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyCheck {
    Idle,
    Running,
    Ok(String),
    Warn(String),
    Failed(String),
}

/// L'esito della prova, con il numero della prova a cui risponde.
pub(super) type CheckOutcome = (u32, Result<Check, String>);

pub struct SettingsForm {
    pub agent_model: String,
    pub agent_effort: String,
    pub agent_fast: bool,
    pub transcribe_model: TextEdit,
    pub transcribe_lang: TextEdit,
    /// Una chiave nuova da salvare; vuota lascia quella di adesso.
    pub new_key: TextEdit,
    pub audio_source: String,
    /// I modelli di Codex (`slug`, nome) e i microfoni, letti all'apertura.
    pub models: Vec<(String, String)>,
    pub sources: Vec<AudioSource>,
    pub sources_error: Option<String>,
    pub default_source: Option<String>,
    pub codex: String,
    pub picker: Option<Picker>,
    pub check: KeyCheck,
    check_id: u32,
    pub error: Option<String>,
    /// Volume d'ingresso e livello del microfono scelto.
    pub mic: MicPanel,
}

/// Il nome del microfono predefinito come lo mostra il menu.
pub const DEFAULT_SOURCE_LABEL: &str = "Predefinito di sistema";

impl SettingsForm {
    fn new(s: &Settings, ffmpeg: &str) -> SettingsForm {
        let mut f = SettingsForm {
            agent_model: s.agent_model.clone(),
            agent_effort: s.agent_effort.clone(),
            agent_fast: s.agent_fast,
            transcribe_model: TextEdit::new(s.transcribe_model.clone()),
            transcribe_lang: TextEdit::new(s.transcribe_lang.clone()),
            new_key: TextEdit::new(""),
            audio_source: s.audio_source.clone(),
            models: settings::agent_models(),
            sources: Vec::new(),
            sources_error: None,
            default_source: None,
            codex: settings::codex_login(&crate::agent::codex_bin()),
            picker: None,
            check: KeyCheck::Idle,
            check_id: 0,
            error: None,
            mic: MicPanel::new("pactl", ffmpeg),
        };
        f.reload_sources();
        f
    }

    pub fn reload_sources(&mut self) {
        match settings::audio_sources() {
            Ok(list) => {
                self.sources = list;
                self.sources_error = None;
            }
            Err(e) => {
                self.sources.clear();
                self.sources_error = Some(e);
            }
        }
        self.default_source = settings::default_source();
    }

    /// Le voci del menu dei modelli (nomi da mostrare) e quella scelta. Un modello che Codex non
    /// elenca più resta scelto e compare per primo.
    pub fn model_options(&self) -> (Vec<String>, String) {
        let mut names: Vec<String> = self.models.iter().map(|(_, n)| n.clone()).collect();
        let current = match self.models.iter().find(|(slug, _)| *slug == self.agent_model) {
            Some((_, n)) => n.clone(),
            None => {
                names.insert(0, self.agent_model.clone());
                self.agent_model.clone()
            }
        };
        (names, current)
    }

    pub fn choose_model(&mut self, label: &str) {
        self.agent_model = self.models.iter().find(|(_, n)| n == label).map_or(label.to_string(), |(slug, _)| slug.clone());
        self.picker = None;
    }

    fn source_label(&self, src: &AudioSource) -> String {
        let twin = self.sources.iter().filter(|o| o.description == src.description).count() > 1;
        let mut label = if twin { format!("{} ({})", src.description, src.name) } else { src.description.clone() };
        if src.muted {
            label.push_str(" · silenziato");
        }
        label
    }

    /// Le voci del menu dei microfoni e quella scelta: prima il predefinito di sistema, poi gli
    /// altri; uno scelto ma scollegato resta in fondo, segnato.
    pub fn source_options(&self) -> (Vec<String>, String) {
        let default_label = match self.default_source.as_deref().and_then(|d| self.sources.iter().find(|s| s.name == d)) {
            Some(d) => format!("{DEFAULT_SOURCE_LABEL} ({})", d.description),
            None => DEFAULT_SOURCE_LABEL.to_string(),
        };
        let mut labels = vec![default_label.clone()];
        labels.extend(self.sources.iter().map(|s| self.source_label(s)));
        let current = if self.audio_source.is_empty() {
            default_label
        } else if let Some(s) = self.sources.iter().find(|s| s.name == self.audio_source) {
            self.source_label(s)
        } else {
            let missing = format!("{} (non collegato)", self.audio_source);
            labels.push(missing.clone());
            missing
        };
        (labels, current)
    }

    pub fn choose_source(&mut self, label: &str) {
        if label.starts_with(DEFAULT_SOURCE_LABEL) {
            self.audio_source.clear();
        } else if let Some(s) = self.sources.iter().find(|s| self.source_label(s) == label) {
            self.audio_source = s.name.clone();
        }
        self.picker = None;
    }

    /// Il microfono scelto è silenziato nel sistema: registrerebbe silenzio.
    pub fn chosen_source_muted(&self) -> bool {
        let name = if self.audio_source.is_empty() { self.default_source.clone() } else { Some(self.audio_source.clone()) };
        name.is_some_and(|n| self.sources.iter().any(|s| s.name == n && s.muted))
    }

    fn settings(&self) -> Settings {
        Settings {
            agent_model: self.agent_model.clone(),
            agent_effort: self.agent_effort.clone(),
            agent_fast: self.agent_fast,
            transcribe_model: self.transcribe_model.text.clone(),
            transcribe_lang: self.transcribe_lang.text.clone(),
            audio_source: self.audio_source.clone(),
        }
        .cleaned()
    }
}

impl State {
    /// Le ultime cifre della chiave in uso, per riconoscerla senza mostrarla.
    pub fn key_tail(&self) -> Option<String> {
        self.gemini_key.as_deref().map(settings::mask)
    }

    pub fn open_settings(&mut self) {
        let mut f = SettingsForm::new(&self.settings, &self.ffmpeg);
        let source = f.audio_source.clone();
        f.mic.listen(&source);
        self.form = Some(f);
    }

    /// Chiude senza salvare: anche il volume del microfono torna com'era.
    pub fn close_settings(&mut self) {
        if let Some(f) = &mut self.form {
            f.mic.restore();
        }
        self.form = None;
    }

    /// Sceglie il microfono nel menu e comincia ad ascoltarlo.
    pub fn choose_source(&mut self, label: &str) {
        if let Some(f) = &mut self.form {
            f.choose_source(label);
            let source = f.audio_source.clone();
            f.mic.listen(&source);
        }
    }

    /// «Aggiorna»: rilegge i microfoni (uno appena collegato) e riascolta quello scelto.
    pub fn reload_mics(&mut self) {
        if let Some(f) = &mut self.form {
            f.reload_sources();
            let source = f.audio_source.clone();
            f.mic.listen(&source);
        }
    }

    /// Il cursore del volume d'ingresso: `now` per il rilascio e i pulsanti.
    pub fn set_mic_volume(&mut self, pct: u32, now: bool) {
        if let Some(f) = &mut self.form {
            let source = f.audio_source.clone();
            f.mic.set_volume(&source, pct, now);
        }
    }

    /// Toglie il silenzio al microfono scelto.
    pub fn unmute_mic(&mut self) {
        if let Some(f) = &mut self.form {
            let source = f.audio_source.clone();
            f.mic.error = settings::unmute_source(&f.mic.pactl, &source).err();
            f.reload_sources();
        }
    }

    /// Da `poll`: manda al sistema l'ultimo volume del cursore rimasto in attesa.
    pub(super) fn flush_mic_volume(&mut self) {
        if let Some(f) = &mut self.form {
            f.mic.flush(false);
        }
    }

    pub fn toggle_picker(&mut self, p: Picker) {
        if let Some(f) = &mut self.form {
            f.picker = if f.picker == Some(p) { None } else { Some(p) };
        }
    }

    pub fn close_picker(&mut self) {
        if let Some(f) = &mut self.form {
            f.picker = None;
        }
    }

    /// Salva le scelte. Una chiave nuova si controlla e si salva a parte, leggibile solo
    /// dall'utente; un microfono nuovo vale subito, anche a registrazione in corso.
    pub fn save_settings(&mut self) {
        let Some(f) = &mut self.form else { return };
        let new_key = f.new_key.text.trim().to_string();
        if !new_key.is_empty()
            && let Some(why) = settings::key_problem(&new_key)
        {
            f.error = Some(format!("Chiave non salvata: {why}."));
            return;
        }
        let next = f.settings();
        if let Some(dir) = &self.config_dir {
            if let Err(e) = next.save(dir) {
                f.error = Some(format!("Non riesco a salvare le impostazioni: {e}"));
                return;
            }
            if !new_key.is_empty()
                && let Err(e) = settings::save_key(dir, &new_key)
            {
                f.error = Some(format!("Non riesco a salvare la chiave: {e}"));
                return;
            }
        }
        let source_changed = next.audio_source != self.settings.audio_source;
        self.settings = next;
        if self.config_dir.is_some() {
            self.refresh_key();
        } else if !new_key.is_empty() {
            // Anteprime e test: la chiave resta in memoria.
            self.gemini_key = Some(new_key.clone());
            self.key_source = KeySource::Saved;
        }
        if !new_key.is_empty() {
            // Con la chiave nuova l'audio in attesa riparte subito.
            self.transcribe_error = None;
            self.tx_retry_at = 0.0;
        }
        if let Some(f) = &mut self.form {
            f.mic.keep();
        }
        self.form = None;
        if source_changed {
            self.restart_recording();
        }
    }

    /// Prova la chiave (quella appena incollata o quella in uso) con il modello scritto nel
    /// pannello, in un thread: Gemini risponde in un attimo e non trascrive niente.
    pub fn check_key(&mut self) {
        let Some(wake) = self.waker.clone() else { return };
        let active = self.gemini_key.clone();
        let tx = self.tx_check.clone();
        let Some(f) = &mut self.form else { return };
        let typed = f.new_key.text.trim().to_string();
        let Some(key) = (if typed.is_empty() { active } else { Some(typed) }) else {
            f.check = KeyCheck::Failed("non c'è nessuna chiave da provare: incollala nel campo qui sopra".into());
            return;
        };
        let model = f.transcribe_model.text.trim().to_string();
        f.check_id += 1;
        f.check = KeyCheck::Running;
        let id = f.check_id;
        std::thread::spawn(move || {
            let _ = tx.send((id, crate::transcribe::check(&key, &model)));
            wake();
        });
    }

    /// Ritira l'esito della prova, se il pannello è ancora aperto su quella prova.
    pub(super) fn collect_key_check(&mut self) -> bool {
        let mut changed = false;
        while let Ok((id, outcome)) = self.rx_check.try_recv() {
            let Some(f) = self.form.as_mut().filter(|f| f.check_id == id) else { continue };
            f.check = match outcome {
                Ok(Check::Ok(m)) => KeyCheck::Ok(m),
                Ok(Check::Warn(m)) => KeyCheck::Warn(m),
                Err(e) => KeyCheck::Failed(e),
            };
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> SettingsForm {
        SettingsForm {
            agent_model: "gpt-6.1-terra".into(),
            agent_effort: "low".into(),
            agent_fast: true,
            transcribe_model: TextEdit::new("gemini-3.5-transcribe"),
            transcribe_lang: TextEdit::new("it-IT"),
            new_key: TextEdit::new(""),
            audio_source: String::new(),
            models: vec![("gpt-6.1-sol".into(), "GPT-6.1-Sol".into()), ("gpt-6.1-terra".into(), "GPT-6.1 Terra".into())],
            sources: vec![
                AudioSource { name: "alsa_input.pci".into(), description: "Built-in Audio".into(), muted: false },
                AudioSource { name: "alsa_input.usb-rode".into(), description: "RØDE NT-USB".into(), muted: true },
            ],
            sources_error: None,
            default_source: Some("alsa_input.pci".into()),
            codex: String::new(),
            picker: None,
            check: KeyCheck::Idle,
            check_id: 0,
            error: None,
            mic: MicPanel::new("/bin/false", "/bin/false"),
        }
    }

    /// Un finto pactl: risponde 47% e scrive su `log` ogni volume che riceve.
    fn fake_pactl(name: &str) -> (String, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rr-pactl-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log.txt");
        let bin = dir.join("pactl.sh");
        std::fs::write(&bin, format!("#!/bin/sh\nif [ \"$1\" = get-source-volume ]; then echo 'Volume: front-left: 30840 /  47% / -19.64 dB'; else echo \"$@\" >> \"{}\"; fi\n", log.display())).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin.to_string_lossy().into_owned(), log)
    }

    fn log_lines(log: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(log).unwrap_or_default().lines().map(String::from).collect()
    }

    #[test]
    fn dragging_the_volume_reaches_the_system_a_few_times_and_the_last_value_always_arrives() {
        let (pactl, log) = fake_pactl("drag");
        let mut m = MicPanel::new(&pactl, "/bin/false");
        m.volume = settings::source_volume(&pactl, "");
        assert_eq!(m.volume, Some(47));
        m.set_volume("", 60, false);
        m.set_volume("", 70, false);
        m.set_volume("", 80, false);
        assert_eq!(m.volume, Some(80), "il cursore segue la mano");
        assert_eq!(log_lines(&log), ["set-source-volume @DEFAULT_SOURCE@ 60%"], "gli altri aspettano il loro turno");
        std::thread::sleep(VOLUME_EVERY + Duration::from_millis(20));
        m.flush(false);
        assert_eq!(log_lines(&log).last().map(String::as_str), Some("set-source-volume @DEFAULT_SOURCE@ 80%"));
        m.set_volume("", 200, true);
        assert_eq!(log_lines(&log).last().map(String::as_str), Some("set-source-volume @DEFAULT_SOURCE@ 150%"), "mai oltre il 150%");
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }

    #[test]
    fn cancelling_puts_the_volume_back_and_saving_keeps_it() {
        let (pactl, log) = fake_pactl("undo");
        let mut s = quiet();
        let mut f = form();
        f.mic = MicPanel::new(&pactl, "/bin/false");
        f.mic.volume = Some(47);
        s.form = Some(f);
        s.set_mic_volume(120, true);
        s.close_settings();
        assert_eq!(log_lines(&log), ["set-source-volume @DEFAULT_SOURCE@ 120%", "set-source-volume @DEFAULT_SOURCE@ 47%"], "annullato: torna al 47%");

        let mut f = form();
        f.mic = MicPanel::new(&pactl, "/bin/false");
        f.mic.volume = Some(47);
        s.form = Some(f);
        s.set_mic_volume(130, true);
        s.save_settings();
        assert!(s.form.is_none());
        assert_eq!(log_lines(&log).last().map(String::as_str), Some("set-source-volume @DEFAULT_SOURCE@ 130%"), "salvato: resta al 130%");
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }

    fn quiet() -> State {
        State::with_dir(std::env::temp_dir().join("rr-settings-form"), false)
    }

    #[test]
    fn the_model_menu_shows_names_and_saves_slugs() {
        let mut f = form();
        let (names, current) = f.model_options();
        assert_eq!(names, ["GPT-6.1-Sol", "GPT-6.1 Terra"]);
        assert_eq!(current, "GPT-6.1 Terra");
        f.choose_model("GPT-6.1-Sol");
        assert_eq!(f.agent_model, "gpt-6.1-sol");
        f.agent_model = "gpt-vecchio".into();
        let (names, current) = f.model_options();
        assert_eq!((names[0].as_str(), current.as_str()), ("gpt-vecchio", "gpt-vecchio"), "un modello non più elencato resta scelto");
    }

    #[test]
    fn the_microphone_menu_starts_with_the_system_default_and_marks_what_is_missing_or_muted() {
        let mut f = form();
        let (labels, current) = f.source_options();
        assert_eq!(labels, ["Predefinito di sistema (Built-in Audio)", "Built-in Audio", "RØDE NT-USB · silenziato"]);
        assert_eq!(current, labels[0]);
        f.choose_source("RØDE NT-USB · silenziato");
        assert_eq!(f.audio_source, "alsa_input.usb-rode");
        assert!(f.chosen_source_muted());
        f.choose_source(&labels[0]);
        assert_eq!(f.audio_source, "", "il predefinito si salva come vuoto: segue il sistema");
        f.audio_source = "alsa_input.usb-scollegato".into();
        let (labels, current) = f.source_options();
        assert_eq!(current, "alsa_input.usb-scollegato (non collegato)");
        assert_eq!(labels.last(), Some(&current));
    }

    #[test]
    fn saving_applies_the_choices_and_cancelling_throws_them_away() {
        let mut s = quiet();
        s.form = Some(form());
        let f = s.form.as_mut().unwrap();
        f.agent_effort = "high".into();
        f.agent_fast = false;
        f.transcribe_lang = TextEdit::new("en-US");
        s.close_settings();
        assert_eq!(s.settings, Settings::default(), "annullato: niente cambia");

        s.form = Some(form());
        let f = s.form.as_mut().unwrap();
        f.agent_effort = "high".into();
        f.agent_model = "gpt-6.1-sol".into();
        f.transcribe_lang = TextEdit::new(" en-US ");
        f.audio_source = "alsa_input.usb-rode".into();
        s.save_settings();
        assert!(s.form.is_none());
        assert_eq!(s.settings.agent_effort, "high");
        assert_eq!(s.settings.transcribe_lang, "en-US");
        assert_eq!(s.settings.audio_source, "alsa_input.usb-rode");
        assert_eq!(s.settings.agent().model, "gpt-6.1-sol");
    }

    #[test]
    fn a_bad_key_keeps_the_panel_open_and_a_good_one_turns_transcription_on() {
        let mut s = quiet();
        s.gemini_key = None;
        s.transcribe_error = Some("rete".into());
        s.form = Some(form());
        s.form.as_mut().unwrap().new_key = TextEdit::new("AQ.con uno spazio dentro-abcdef");
        s.save_settings();
        assert!(s.form.as_ref().unwrap().error.as_deref().is_some_and(|e| e.contains("Chiave non salvata")));
        assert!(s.transcription_off());
        s.form.as_mut().unwrap().new_key = TextEdit::new("  AQ.chiave-buona-abbastanza-lunga ");
        s.save_settings();
        assert!(s.form.is_none());
        assert!(!s.transcription_off());
        assert_eq!(s.key_source, KeySource::Saved);
        assert_eq!(s.transcribe_error, None, "l'audio in attesa riparte");
    }

    #[test]
    fn a_check_answer_for_an_older_check_is_ignored() {
        let mut s = quiet();
        s.form = Some(form());
        s.form.as_mut().unwrap().check_id = 2;
        s.form.as_mut().unwrap().check = KeyCheck::Running;
        s.tx_check.send((1, Ok(Check::Ok("vecchia".into())))).unwrap();
        s.tx_check.send((2, Err("chiave rifiutata".into()))).unwrap();
        assert!(s.collect_key_check());
        assert_eq!(s.form.as_ref().unwrap().check, KeyCheck::Failed("chiave rifiutata".into()));
    }
}
