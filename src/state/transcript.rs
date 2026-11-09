//! Dal microfono alla trascrizione: raccoglie i pezzi di audio che ffmpeg ha chiuso, li manda a
//! Gemini a finestre di dieci minuti e rimette il testo nell'area giusta.
//!
//! Dentro una finestra tutti i pezzi chiusi finora vanno in un'unica richiesta, così le voci sono
//! coerenti; a ogni passata il testo della finestra sostituisce quello della passata prima.

use rune::Tx;

use super::{Rec, State};
use crate::model::{AudioSegment, CHUNK_SECS, Speaker, Turn};
use crate::transcribe::{self, Job, Utterance};

/// Secondi minimi tra due richieste mentre si registra (a registrazione ferma si va subito).
const GAP: f64 = 30.0;
/// Attesa dopo un errore, prima di riprovare.
const RETRY: f64 = 45.0;

pub(super) struct InFlight {
    /// I pezzi (`audio/…ogg`) che questa richiesta copre.
    pub chunks: Vec<String>,
}

impl State {
    /// Porta nell'area i pezzi che ffmpeg ha finito di scrivere. Un pezzo è chiuso quando c'è il
    /// successivo, o quando la registrazione è finita (`total_secs`). Torna true se ne ha aggiunti.
    pub(super) fn harvest(&mut self, total_secs: Option<f32>) -> bool {
        let audio = self.dir.join("audio");
        let base = self.rec_base.clone();
        let path = |i: u32| audio.join(format!("{base}_{i:03}.ogg"));
        let mut idx = self.rec_next_idx;
        let mut found = Vec::new();
        loop {
            let file = path(idx);
            if !file.exists() {
                break;
            }
            let next_exists = path(idx + 1).exists();
            if !next_exists && total_secs.is_none() {
                break;
            }
            let secs = match (next_exists, total_secs) {
                (false, Some(total)) => (total - (idx * CHUNK_SECS) as f32).clamp(0.5, CHUNK_SECS as f32),
                _ => CHUNK_SECS as f32,
            };
            if std::fs::metadata(&file).is_ok_and(|m| m.len() > 0) {
                found.push(AudioSegment { file: format!("audio/{base}_{idx:03}.ogg"), secs, started: self.rec_started + (idx * CHUNK_SECS) as u64, transcribed: false });
            }
            idx += 1;
        }
        self.rec_next_idx = idx;
        if found.is_empty() {
            return false;
        }
        self.rec_harvested += found.iter().map(|s| s.secs).sum::<f32>();
        let area = self.rec_area;
        if let Some(a) = self.area_mut(area) {
            a.segments.extend(found);
        }
        self.touch();
        true
    }

    /// La prossima finestra da trascrivere: la più vecchia con pezzi non ancora trascritti.
    fn next_job(&self) -> Option<(Job, Vec<String>)> {
        let key = self.gemini_key.clone()?;
        let sessions = std::iter::once(&self.session).chain(self.slots.iter().flatten().map(|p| &p.session));
        for s in sessions {
            let Some(window) = s.segments.iter().find(|g| !g.transcribed).and_then(|g| g.window()) else { continue };
            let mut chunks: Vec<&AudioSegment> = s.segments.iter().filter(|g| g.window().as_deref() == Some(window.as_str())).collect();
            chunks.sort_by_key(|g| g.index());
            let names = chunks.iter().map(|g| g.file.clone()).collect();
            let files = chunks.iter().map(|g| self.dir.join(&g.file)).collect();
            let job = Job {
                area: s.id,
                window,
                files,
                work_dir: self.dir.join(".trascrizione"),
                key,
                model: self.settings.transcribe_model.clone(),
                lang: self.settings.transcribe_lang.clone(),
            };
            return Some((job, names));
        }
        None
    }

    /// Una volta per frame: ritira gli esiti e, se è il momento, manda la prossima finestra.
    pub(super) fn pump_transcription(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        while let Ok((area, window, outcome)) = self.rx_out.try_recv() {
            self.finish_transcription(area, &window, outcome, now);
            changed = true;
        }
        if self.transcribing.is_some() || self.waker.is_none() {
            return changed;
        }
        let Some((job, chunks)) = self.next_job() else { return changed };
        let due = if self.rec == Rec::Live { (self.tx_last_start + GAP).max(self.tx_retry_at) } else { self.tx_retry_at };
        if now < due {
            tx.wake_at(due);
            return changed;
        }
        let Some(wake) = self.waker.clone() else { return changed };
        self.transcribing = Some(InFlight { chunks });
        self.tx_last_start = now;
        transcribe::spawn(job, self.tx_out.clone(), wake);
        true
    }

    pub(super) fn finish_transcription(&mut self, area: u32, window: &str, outcome: Result<Vec<Utterance>, String>, now: f64) {
        let flight = self.transcribing.take();
        match outcome {
            Ok(utterances) => {
                self.transcribe_error = None;
                let chunks = flight.map(|f| f.chunks).unwrap_or_default();
                self.with_area(area, |st| st.apply_transcript(window, &utterances, &chunks));
            }
            Err(e) => {
                self.transcribe_error = Some(e);
                self.tx_retry_at = now + RETRY;
            }
        }
    }

    /// Mette il testo di una finestra nell'area in vista e segna i suoi pezzi come trascritti.
    fn apply_transcript(&mut self, window: &str, utterances: &[Utterance], chunks: &[String]) {
        let swapped = self.session.voice_swaps.iter().any(|w| w == window);
        // La prima voce a parlare nella finestra è «Io», finché non la scambi a mano.
        let role = |speaker: u32| if (speaker == 0) != swapped { Speaker::Io } else { Speaker::Cliente };
        let turns: Vec<Turn> = utterances.iter().map(|u| Turn { speaker: role(u.speaker), text: u.text.clone(), source: Some(window.to_string()) }).collect();
        if self.session.replace_source_turns(window, turns) {
            self.fresh = true;
        }
        for seg in self.session.segments.iter_mut().filter(|g| chunks.contains(&g.file)) {
            seg.transcribed = true;
        }
        self.touch();
    }

    /// Scambia «Io» e «Cliente» in una finestra, quando le voci sono state attribuite al contrario.
    pub fn swap_voices(&mut self, source: &str) {
        self.session.swap_voices(source);
        self.fresh = true;
        self.touch();
    }

    pub fn transcribing_now(&self) -> bool {
        self.transcribing.is_some()
    }

    /// Manca la chiave: l'audio si registra ma non diventa testo.
    pub fn transcription_off(&self) -> bool {
        self.gemini_key.is_none()
    }

    /// Audio registrato che aspetta ancora di diventare testo, in tutte le aree.
    pub fn pending_audio_secs(&self) -> f32 {
        let parked: f32 = self.slots.iter().flatten().map(|p| p.session.pending_audio_secs()).sum();
        parked + self.session.pending_audio_secs()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rr-tx-{name}-{}", std::process::id()))
    }

    fn state(name: &str) -> State {
        let mut s = State::with_dir(temp(name), false);
        s.gemini_key = Some("chiave".into());
        s
    }

    fn touch_chunk(s: &State, idx: u32, bytes: usize) {
        let dir = s.dir.join("audio");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}_{idx:03}.ogg", s.rec_base)), vec![1u8; bytes]).unwrap();
    }

    fn seg(file: &str, transcribed: bool) -> AudioSegment {
        AudioSegment { file: file.into(), secs: 20.0, started: 0, transcribed }
    }

    fn u(speaker: u32, text: &str) -> Utterance {
        Utterance { speaker, text: text.into() }
    }

    #[test]
    fn a_chunk_is_taken_only_once_the_next_one_has_started() {
        let mut s = state("harvest");
        s.rec_base = "a0-100".into();
        s.rec_started = 100;
        touch_chunk(&s, 0, 10);
        assert!(!s.harvest(None), "l'unico pezzo è ancora in scrittura");
        touch_chunk(&s, 1, 10);
        touch_chunk(&s, 2, 10);
        assert!(s.harvest(None));
        assert_eq!(s.session.segments.len(), 2, "il terzo è ancora aperto");
        assert_eq!(s.session.segments[1].started, 120);
        assert!(s.session.segments.iter().all(|g| !g.transcribed && (g.secs - 20.0).abs() < 0.01));
        assert!(!s.harvest(None), "niente di nuovo");
        let _ = std::fs::remove_dir_all(temp("harvest"));
    }

    #[test]
    fn when_the_recording_ends_the_last_chunk_gets_its_real_length() {
        let mut s = state("harvest-end");
        s.rec_base = "a0-100".into();
        touch_chunk(&s, 0, 10);
        touch_chunk(&s, 1, 10);
        touch_chunk(&s, 2, 10);
        assert!(s.harvest(Some(47.0)));
        let secs: Vec<f32> = s.session.segments.iter().map(|g| g.secs).collect();
        assert_eq!(secs, [20.0, 20.0, 7.0]);
        assert!((s.rec_harvested - 47.0).abs() < 0.01, "il contatore non conta due volte");
        let _ = std::fs::remove_dir_all(temp("harvest-end"));
    }

    #[test]
    fn an_empty_last_chunk_is_not_recorded() {
        let mut s = state("harvest-empty");
        s.rec_base = "a0-100".into();
        touch_chunk(&s, 0, 10);
        touch_chunk(&s, 1, 0);
        s.harvest(Some(21.0));
        assert_eq!(s.session.segments.len(), 1);
        let _ = std::fs::remove_dir_all(temp("harvest-empty"));
    }

    #[test]
    fn the_next_job_is_the_oldest_window_with_something_missing_and_includes_all_its_chunks() {
        let mut s = state("job");
        s.session.segments = vec![
            seg("audio/a0-100_000.ogg", true),
            seg("audio/a0-100_001.ogg", true),
            seg("audio/a0-100_002.ogg", false),
            seg("audio/a0-100_030.ogg", false),
        ];
        let (job, names) = s.next_job().unwrap();
        assert_eq!(job.window, "a0-100w0");
        assert_eq!(names, ["audio/a0-100_000.ogg", "audio/a0-100_001.ogg", "audio/a0-100_002.ogg"], "anche quelli già trascritti: la finestra si rifà intera");
        assert_eq!(job.files.len(), 3);
        s.session.segments.iter_mut().take(3).for_each(|g| g.transcribed = true);
        assert_eq!(s.next_job().unwrap().0.window, "a0-100w1");
        s.session.segments.iter_mut().for_each(|g| g.transcribed = true);
        assert!(s.next_job().is_none());
    }

    #[test]
    fn without_a_key_nothing_is_sent() {
        let mut s = state("nokey");
        s.gemini_key = None;
        s.session.segments = vec![seg("audio/a0-100_000.ogg", false)];
        assert!(s.next_job().is_none());
        assert!(s.transcription_off());
    }

    #[test]
    fn the_first_voice_is_me_until_i_swap_and_a_new_pass_replaces_the_text_in_place() {
        let mut s = state("apply");
        s.session.segments = vec![seg("audio/a0-100_000.ogg", false)];
        let chunks = vec!["audio/a0-100_000.ogg".to_string()];
        s.apply_transcript("a0-100w0", &[u(0, "Buongiorno"), u(1, "Salve")], &chunks);
        assert_eq!(s.session.turns[0].speaker, Speaker::Io);
        assert_eq!(s.session.turns[1].speaker, Speaker::Cliente);
        assert!(s.fresh, "l'agente ha del materiale nuovo");
        assert!(s.session.segments[0].transcribed);

        s.swap_voices("a0-100w0");
        assert_eq!(s.session.turns[0].speaker, Speaker::Cliente);
        s.apply_transcript("a0-100w0", &[u(0, "Buongiorno a tutti"), u(1, "Salve")], &chunks);
        assert_eq!(s.session.turns.len(), 2, "sostituisce, non aggiunge");
        assert_eq!(s.session.turns[0].text, "Buongiorno a tutti");
        assert_eq!(s.session.turns[0].speaker, Speaker::Cliente, "lo scambio vale anche per le passate successive");
    }

    #[test]
    fn a_failed_pass_keeps_the_audio_pending_and_waits_before_retrying() {
        let mut s = state("fail");
        s.session.segments = vec![seg("audio/a0-100_000.ogg", false)];
        s.transcribing = Some(InFlight { chunks: vec!["audio/a0-100_000.ogg".into()] });
        s.finish_transcription(0, "a0-100w0", Err("rete: timeout".into()), 100.0);
        assert!(s.transcribing.is_none());
        assert_eq!(s.transcribe_error.as_deref(), Some("rete: timeout"));
        assert!(!s.session.segments[0].transcribed);
        assert_eq!(s.tx_retry_at, 145.0);

        s.transcribing = Some(InFlight { chunks: vec!["audio/a0-100_000.ogg".into()] });
        s.finish_transcription(0, "a0-100w0", Ok(vec![u(0, "ok")]), 150.0);
        assert!(s.transcribe_error.is_none(), "un successo cancella l'errore");
        assert!(s.session.segments[0].transcribed);
    }

    #[test]
    fn text_for_an_area_that_is_no_longer_open_lands_there() {
        let mut s = state("parked");
        s.session.segments = vec![seg("audio/a0-100_000.ogg", false)];
        let first = s.session.id;
        s.add_area();
        s.transcribing = Some(InFlight { chunks: vec!["audio/a0-100_000.ogg".into()] });
        s.finish_transcription(first, "a0-100w0", Ok(vec![u(0, "Detto prima")]), 10.0);
        assert!(s.session.turns.is_empty(), "l'area in vista non cambia");
        assert!(!s.fresh);
        s.switch_area(0);
        assert_eq!(s.session.turns.len(), 1);
        assert!(s.session.segments[0].transcribed);
        assert_eq!(s.pending_audio_secs(), 0.0);
        let _ = std::fs::remove_dir_all(temp("parked"));
    }

    #[test]
    fn pending_audio_adds_up_across_areas() {
        let mut s = state("pending");
        s.session.segments = vec![seg("audio/a0-1_000.ogg", false), seg("audio/a0-1_001.ogg", true)];
        s.add_area();
        s.session.segments = vec![seg("audio/a1-2_000.ogg", false)];
        assert_eq!(s.pending_audio_secs(), 40.0);
        let _ = std::fs::remove_dir_all(temp("pending"));
    }
}
