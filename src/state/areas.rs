//! Le aree di lavoro (un argomento e un interlocutore ciascuna) e la registrazione audio.
//!
//! L'area in vista vive in `State::session` e nei campi di modifica accanto; le altre stanno
//! parcheggiate in `slots`. Cambiare area le scambia, senza perdere note, risposte a metà né
//! modifiche in volo.

use std::mem;

use rune::{TextEdit, Tx};

use super::{State, unix_secs};
use crate::agent::Reply;
use crate::audio::Recorder;
use crate::model::{Project, Session};

/// Una zona di lavoro non in vista, con quello che serve per riprenderla com'era.
pub(super) struct Parked {
    pub(super) session: Session,
    notes_edit: TextEdit,
    interlocutor_edit: TextEdit,
    answer_edits: std::collections::HashMap<u32, TextEdit>,
    side_answers: std::collections::HashMap<u32, TextEdit>,
    fresh: bool,
    in_flight: Vec<u32>,
}

impl Parked {
    fn new(session: Session) -> Parked {
        let answer_edits = session.questions.iter().map(|q| (q.id, TextEdit::new(""))).collect();
        Parked {
            notes_edit: TextEdit::new(session.notes.clone()),
            interlocutor_edit: TextEdit::new(session.interlocutor.clone()),
            session,
            answer_edits,
            side_answers: std::collections::HashMap::new(),
            fresh: false,
            in_flight: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rec {
    /// Non si è ancora registrato niente in questa sessione.
    Idle,
    Live,
    Paused,
}

/// Quello che serve alla barra delle aree.
#[derive(Clone, Debug)]
pub struct AreaInfo {
    pub index: usize,
    pub name: String,
    /// Domande ancora senza risposta.
    pub pending: usize,
    /// Si sta registrando proprio in quest'area.
    pub recording: bool,
}

impl State {
    // ---- aree ----

    pub fn project(&self) -> Project {
        let areas = (0..self.slots.len())
            .filter_map(|i| if i == self.active { Some(self.session.clone()) } else { self.slots[i].as_ref().map(|p| p.session.clone()) })
            .collect();
        Project { client: self.client.clone(), active: self.active, next_area_id: self.next_area_id, areas }
    }

    /// Mette in vista un progetto caricato o preparato.
    pub fn adopt_project(&mut self, p: Project) {
        let p = if p.areas.is_empty() { Project::single("Area 1") } else { p };
        let active = p.active.min(p.areas.len() - 1);
        self.client = p.client.clone();
        self.client_edit = TextEdit::new(p.client);
        self.next_area_id = p.next_area_id;
        self.active = active;
        self.slots = p.areas.iter().map(|_| None).collect();
        let mut live = None;
        for (i, area) in p.areas.into_iter().enumerate() {
            if i == active {
                live = Some(area);
            } else {
                self.slots[i] = Some(Parked::new(area));
            }
        }
        let live = Parked::new(live.unwrap_or_default());
        self.session = live.session;
        self.notes_edit = live.notes_edit;
        self.interlocutor_edit = live.interlocutor_edit;
        self.answer_edits = live.answer_edits;
        self.side_answers = live.side_answers;
        self.fresh = false;
        self.in_flight.clear();
        self.load_all_thumbs();
    }

    pub fn areas(&self) -> Vec<AreaInfo> {
        let live = self.rec == Rec::Live;
        (0..self.slots.len())
            .filter_map(|i| {
                let s = if i == self.active { Some(&self.session) } else { self.slots[i].as_ref().map(|p| &p.session) }?;
                Some(AreaInfo { index: i, name: s.name.clone(), pending: s.pending_count(), recording: live && self.rec_area == s.id })
            })
            .collect()
    }

    /// Porta le modifiche dei campi di testo dentro l'area; true se qualcosa è cambiato.
    pub(super) fn sync_edits(&mut self) -> bool {
        let mut changed = false;
        if self.notes_edit.text != self.session.notes {
            self.session.notes = self.notes_edit.text.clone();
            self.fresh = true;
            changed = true;
        }
        if self.interlocutor_edit.text != self.session.interlocutor {
            self.session.interlocutor = self.interlocutor_edit.text.clone();
            changed = true;
        }
        changed
    }

    /// Scambia l'area in vista con la `i`-esima, senza altri effetti: serve anche per consegnare
    /// il risultato dell'agente a un'area che non è più in vista.
    pub(super) fn swap_to(&mut self, i: usize) {
        if i == self.active || i >= self.slots.len() {
            return;
        }
        let Some(next) = self.slots[i].take() else { return };
        let prev = Parked {
            session: mem::replace(&mut self.session, next.session),
            notes_edit: mem::replace(&mut self.notes_edit, next.notes_edit),
            interlocutor_edit: mem::replace(&mut self.interlocutor_edit, next.interlocutor_edit),
            answer_edits: mem::replace(&mut self.answer_edits, next.answer_edits),
            side_answers: mem::replace(&mut self.side_answers, next.side_answers),
            fresh: mem::replace(&mut self.fresh, next.fresh),
            in_flight: mem::replace(&mut self.in_flight, next.in_flight),
        };
        self.slots[self.active] = Some(prev);
        self.active = i;
    }

    /// Prepara l'uscita dall'area in vista; true se si stava registrando.
    fn leave_area(&mut self) -> bool {
        self.finish_rename();
        self.sync_edits();
        // Le novità di quest'area partono verso l'agente prima di lasciarla (se è libero).
        if self.auto && self.fresh {
            self.start_round();
        }
        let was_live = self.rec == Rec::Live;
        if was_live {
            self.end_segment();
        }
        self.editing = None;
        self.node_edit = TextEdit::new("");
        // Ogni area ha il suo diagramma: si riparte adattati.
        self.zoom = super::Zoom::Fit;
        was_live
    }

    fn enter_area(&mut self, was_live: bool) {
        if was_live {
            self.start_recording();
        }
        self.touch();
    }

    /// Passa a un'altra area. Se si sta registrando, l'audio continua nella nuova area.
    pub fn switch_area(&mut self, i: usize) {
        if i == self.active || i >= self.slots.len() {
            return;
        }
        let was_live = self.leave_area();
        self.swap_to(i);
        self.enter_area(was_live);
    }

    /// Crea un'area vuota, ci passa e apre il campo per darle il nome.
    pub fn add_area(&mut self) {
        let was_live = self.leave_area();
        let id = self.next_area_id;
        self.next_area_id += 1;
        let prev = Parked {
            session: mem::replace(&mut self.session, Session::area(id, &format!("Area {}", self.slots.len() + 1))),
            notes_edit: mem::replace(&mut self.notes_edit, TextEdit::new("")),
            interlocutor_edit: mem::replace(&mut self.interlocutor_edit, TextEdit::new("")),
            answer_edits: mem::take(&mut self.answer_edits),
            side_answers: mem::take(&mut self.side_answers),
            fresh: mem::replace(&mut self.fresh, false),
            in_flight: mem::take(&mut self.in_flight),
        };
        self.slots[self.active] = Some(prev);
        self.slots.push(None);
        self.active = self.slots.len() - 1;
        self.enter_area(was_live);
        self.start_rename();
    }

    pub fn start_rename(&mut self) {
        self.renaming = true;
        self.name_edit = TextEdit::new(self.session.name.clone());
        self.name_edit.focused = true;
    }

    /// Conferma il nome; un nome vuoto lascia quello di prima.
    pub fn finish_rename(&mut self) {
        if !self.renaming {
            return;
        }
        self.renaming = false;
        let name = self.name_edit.text.trim().to_string();
        if !name.is_empty() && name != self.session.name {
            self.session.name = name;
            self.touch();
        }
    }

    pub fn cancel_rename(&mut self) {
        self.renaming = false;
    }

    /// Consegna il risultato dell'agente all'area per cui l'aveva chiesto.
    pub(super) fn deliver(&mut self, area: u32, reply: Reply) -> Result<(), String> {
        if area == self.session.id {
            return self.apply_reply(reply);
        }
        self.with_area(area, |st| st.apply_reply(reply)).unwrap_or_else(|| Err("l'area per cui l'agente lavorava non esiste più".into()))
    }

    /// Esegue `f` con l'area `id` in vista e poi rimette a posto quella di prima: serve a
    /// consegnare un risultato a un'area che nel frattempo non è più quella aperta.
    pub(super) fn with_area<R>(&mut self, id: u32, f: impl FnOnce(&mut State) -> R) -> Option<R> {
        if self.session.id == id {
            return Some(f(self));
        }
        let i = self.slots.iter().position(|s| s.as_ref().is_some_and(|p| p.session.id == id))?;
        let back = self.active;
        self.swap_to(i);
        let out = f(self);
        self.swap_to(back);
        Some(out)
    }

    pub(super) fn area_mut(&mut self, id: u32) -> Option<&mut Session> {
        if self.session.id == id {
            return Some(&mut self.session);
        }
        self.slots.iter_mut().flatten().map(|p| &mut p.session).find(|s| s.id == id)
    }

    // ---- registrazione ----

    /// Il pulsante: registra, ferma, riprende.
    pub fn toggle_recording(&mut self) {
        match self.rec {
            Rec::Live => self.pause_recording(),
            Rec::Idle | Rec::Paused => self.start_recording(),
        }
    }

    /// Per le anteprime: mostra «in registrazione» senza aprire il microfono.
    pub fn start_demo_recording(&mut self) {
        self.rec = Rec::Live;
        self.rec_area = self.session.id;
        self.seg_t0 = self.now;
    }

    pub fn pause_recording(&mut self) {
        self.end_segment();
        self.rec = Rec::Paused;
        self.touch();
    }

    pub(super) fn start_recording(&mut self) {
        if self.recorder.is_some() {
            return;
        }
        let audio = self.dir.join("audio");
        let mut started = unix_secs();
        let mut base = format!("a{}-{started}", self.session.id);
        while audio.join(format!("{base}_000.ogg")).exists() {
            started += 1;
            base = format!("a{}-{started}", self.session.id);
        }
        let (source, warning) = self.recording_source();
        match Recorder::start(&self.ffmpeg, &source, &audio, &base, &audio.join("ffmpeg.log")) {
            Ok(r) => {
                self.recorder = Some(r);
                self.rec = Rec::Live;
                self.rec_area = self.session.id;
                self.rec_base = base;
                self.rec_next_idx = 0;
                self.rec_harvested = 0.0;
                self.rec_started = started;
                self.seg_t0 = self.now;
                self.audio_error = warning;
            }
            Err(e) => {
                self.audio_error = Some(format!("non riesco ad avviare la registrazione: {e}"));
                if self.rec != Rec::Idle {
                    self.rec = Rec::Paused;
                }
            }
        }
    }

    /// Il microfono da cui registrare. Se quello scelto non è collegato (un microfono esterno
    /// rimasto a casa) registra dal predefinito e lo dice, invece di non registrare niente.
    fn recording_source(&self) -> (String, Option<String>) {
        let want = self.settings.source_for_ffmpeg();
        if want == "default" {
            return (want, None);
        }
        match crate::settings::audio_sources() {
            Ok(list) if !list.iter().any(|s| s.name == want) => {
                ("default".into(), Some("Il microfono scelto nelle impostazioni non è collegato: registro da quello predefinito.".into()))
            }
            _ => (want, None),
        }
    }

    /// Cambia microfono senza fermarsi: chiude il pezzo in corso e riparte dal nuovo, nella stessa area.
    pub(super) fn restart_recording(&mut self) {
        if self.rec == Rec::Live && self.recorder.is_some() {
            self.end_segment();
            self.start_recording();
        }
    }

    /// Chiude la registrazione in corso: ffmpeg finisce l'ultimo pezzo e questo va nell'area a cui apparteneva.
    pub(super) fn end_segment(&mut self) {
        let Some(r) = self.recorder.take() else { return };
        let secs = r.stop();
        self.harvest(Some(secs));
        if self.rec_harvested == 0.0 && secs >= 1.0 {
            self.audio_error = Some("la registrazione non ha prodotto audio (vedi audio/ffmpeg.log)".into());
        }
        self.touch();
    }

    /// Se ffmpeg è morto da solo (microfono assente o occupato) mette in pausa e lo dice.
    pub(super) fn check_recorder(&mut self) -> bool {
        if self.rec != Rec::Live || !self.recorder.as_mut().is_some_and(|r| !r.alive()) {
            return false;
        }
        self.end_segment();
        self.rec = Rec::Paused;
        self.audio_error = Some("La registrazione si è fermata da sola: controlla il microfono (audio/ffmpeg.log).".into());
        true
    }

    /// Una volta per frame: aggiorna il contatore ogni secondo mentre si registra.
    pub(super) fn tick_recording(&mut self, now: f64, tx: &mut Tx) -> bool {
        if self.rec != Rec::Live {
            return false;
        }
        let mut changed = self.check_recorder();
        changed |= self.harvest(None);
        let sec = now as i64;
        if sec != self.last_tick {
            self.last_tick = sec;
            changed = true;
        }
        if self.rec == Rec::Live {
            tx.wake_at((sec + 1) as f64);
        }
        changed
    }

    /// Secondi registrati nell'area in vista, compreso il segmento in corso.
    pub fn rec_secs(&self) -> f32 {
        // I pezzi già chiusi stanno in `audio_secs`: del tempo trascorso resta solo quello del pezzo aperto.
        let running = if self.rec == Rec::Live && self.rec_area == self.session.id { ((self.now - self.seg_t0) as f32 - self.rec_harvested).max(0.0) } else { 0.0 };
        self.session.audio_secs() + running
    }

    /// Si sta registrando proprio nell'area in vista.
    pub fn recording_here(&self) -> bool {
        self.rec == Rec::Live && self.rec_area == self.session.id
    }

    pub fn area_name(&self, id: u32) -> Option<String> {
        self.areas().into_iter().zip(0..).find_map(|(a, _)| {
            let s = if a.index == self.active { Some(&self.session) } else { self.slots[a.index].as_ref().map(|p| &p.session) }?;
            (s.id == id).then(|| a.name.clone())
        })
    }

    /// Il nome dell'area in cui si sta registrando adesso.
    pub fn rec_area_name(&self) -> Option<String> {
        if self.rec != Rec::Live {
            return None;
        }
        self.areas().into_iter().find(|a| a.recording).map(|a| a.name)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if self.recorder.is_some() {
            self.end_segment();
            self.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::agent::NewQuestion;
    use crate::audio::tests::fake_ffmpeg;
    use crate::model::Speaker;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rr-areas-{name}-{}", std::process::id()))
    }

    fn quiet(name: &str) -> State {
        State::with_dir(temp(name), false)
    }

    fn with_fake_mic(name: &str) -> State {
        let mut s = quiet(name);
        s.ffmpeg = fake_ffmpeg(&temp(name));
        s
    }

    fn reply(mermaid: &str, questions: Vec<NewQuestion>) -> Reply {
        Reply { summary: "s".into(), mermaid: mermaid.into(), open_points: vec![], questions, attachments: vec![] }
    }

    fn clean(name: &str) {
        let _ = std::fs::remove_dir_all(temp(name));
    }

    #[test]
    fn a_new_area_starts_empty_and_the_other_one_keeps_its_content() {
        let mut s = quiet("new");
        s.session.add_turn(Speaker::Io, "Parliamo di CRM");
        s.notes_edit.text = "nota sul CRM".into();
        s.sync_edits();
        s.add_area();
        assert_eq!(s.areas().len(), 2);
        assert_eq!(s.active, 1);
        assert!(s.session.turns.is_empty() && s.session.mermaid.is_empty());
        assert_eq!(s.notes_edit.text, "");
        assert!(s.renaming, "il nome si sceglie subito");
        s.switch_area(0);
        assert_eq!(s.session.turns.len(), 1);
        assert_eq!(s.notes_edit.text, "nota sul CRM");
    }

    #[test]
    fn a_new_area_starts_fitted_even_if_the_old_one_was_zoomed() {
        let mut s = quiet("zoomreset");
        s.zoom_in();
        s.add_area();
        assert_eq!(s.zoom, super::super::Zoom::Fit);
    }

    #[test]
    fn switching_back_restores_half_typed_answers_and_the_interlocutor() {
        let mut s = quiet("restore");
        let q = s.session.add_question(Some("n1".into()), "Chi?".into());
        s.answer_edits.insert(q, TextEdit::new("Credo la segret"));
        s.side_answers.insert(q, TextEdit::new("Dalla scheda"));
        s.interlocutor_edit.text = "Marco".into();
        s.sync_edits();
        s.add_area();
        assert!(s.answer_edits.is_empty());
        assert_eq!(s.interlocutor_edit.text, "");
        s.switch_area(0);
        assert_eq!(s.answer_edits[&q].text, "Credo la segret");
        assert_eq!(s.side_answers[&q].text, "Dalla scheda", "anche la bozza della scheda Domande torna con la sua area");
        assert_eq!(s.session.interlocutor, "Marco");
    }

    #[test]
    fn renaming_keeps_the_old_name_when_the_new_one_is_empty() {
        let mut s = quiet("rename");
        s.start_rename();
        s.name_edit.text = "  Automazione interna ".into();
        s.finish_rename();
        assert_eq!(s.session.name, "Automazione interna");
        s.start_rename();
        s.name_edit.text = "   ".into();
        s.finish_rename();
        assert_eq!(s.session.name, "Automazione interna");
        s.start_rename();
        s.name_edit.text = "Altro".into();
        s.cancel_rename();
        assert_eq!(s.session.name, "Automazione interna");
    }

    #[test]
    fn leaving_an_area_commits_a_rename_in_progress() {
        let mut s = quiet("rename-leave");
        s.add_area();
        s.name_edit.text = "Sito web".into();
        s.switch_area(0);
        assert_eq!(s.areas()[1].name, "Sito web");
    }

    #[test]
    fn a_reply_for_another_area_lands_there_and_leaves_the_view_untouched() {
        let mut s = quiet("deliver");
        let first = s.session.id;
        s.add_area();
        s.toggle_edit("n1");
        s.node_edit.text = "scrivo".into();
        let q = NewQuestion { node_id: Some("n1".into()), text: "Chi?".into() };
        s.deliver(first, reply("flowchart TD\n n1[Uno] --> n2[Due]", vec![q])).unwrap();
        assert_eq!(s.active, 1, "l'area in vista non cambia");
        assert!(s.session.mermaid.is_empty() && s.session.questions.is_empty());
        assert_eq!(s.editing.as_deref(), Some("n1"), "né l'editor aperto");
        assert_eq!(s.node_edit.text, "scrivo");
        s.switch_area(0);
        assert!(s.session.mermaid.contains("n1[Uno]"));
        assert_eq!(s.session.questions.len(), 1);
        assert!(s.answer_edits.contains_key(&s.session.questions[0].id));
    }

    #[test]
    fn a_reply_for_a_missing_area_is_an_error() {
        let mut s = quiet("missing");
        assert!(s.deliver(99, reply("flowchart TD\n n1[Uno]", vec![])).is_err());
    }

    #[test]
    fn the_whole_project_survives_a_save_and_reload_on_the_active_area() {
        let mut s = quiet("project");
        s.client = "Rossi".into();
        s.session.name = "CRM".into();
        s.add_area();
        s.name_edit.text = "Automazione".into();
        s.finish_rename();
        s.session.interlocutor = "Laura".into();
        let p = s.project();
        let mut t = quiet("project-2");
        t.adopt_project(p);
        assert_eq!(t.client, "Rossi");
        assert_eq!(t.active, 1);
        assert_eq!(t.session.name, "Automazione");
        assert_eq!(t.interlocutor_edit.text, "Laura");
        assert_eq!(t.areas().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["CRM", "Automazione"]);
        t.add_area();
        assert_eq!(t.session.id, 2, "gli id non si riusano");
    }

    #[test]
    fn pausing_and_resuming_make_separate_segments_in_the_same_area() {
        let mut s = with_fake_mic("pause");
        assert_eq!(s.rec, Rec::Idle);
        s.toggle_recording();
        assert_eq!(s.rec, Rec::Live);
        std::thread::sleep(std::time::Duration::from_millis(300));
        s.toggle_recording();
        assert_eq!(s.rec, Rec::Paused);
        assert_eq!(s.session.segments.len(), 1);
        s.toggle_recording();
        assert_eq!(s.rec, Rec::Live);
        s.toggle_recording();
        assert_eq!(s.session.segments.len(), 2);
        assert_ne!(s.session.segments[0].file, s.session.segments[1].file, "mai sovrascrivere");
        assert!(s.dir.join(&s.session.segments[0].file).exists());
        clean("pause");
    }

    #[test]
    fn switching_area_while_recording_continues_in_the_new_area() {
        let mut s = with_fake_mic("switch-live");
        let first = s.session.id;
        s.toggle_recording();
        s.add_area();
        let second = s.session.id;
        assert_eq!(s.rec, Rec::Live, "la registrazione non si interrompe");
        assert_eq!(s.rec_area, second);
        assert!(s.areas()[1].recording && !s.areas()[0].recording);
        s.toggle_recording();
        assert_eq!(s.session.segments.len(), 1, "la parte nuova è nell'area nuova");
        s.switch_area(0);
        assert_eq!(s.session.id, first);
        assert_eq!(s.session.segments.len(), 1, "e quella vecchia in quella vecchia");
        clean("switch-live");
    }

    #[test]
    fn switching_area_while_paused_does_not_start_recording() {
        let mut s = with_fake_mic("switch-paused");
        s.toggle_recording();
        s.toggle_recording();
        s.add_area();
        assert_eq!(s.rec, Rec::Paused);
        assert!(s.recorder.is_none());
        clean("switch-paused");
    }

    #[test]
    fn a_microphone_that_cannot_start_leaves_a_message_and_does_not_pretend_to_record() {
        let mut s = quiet("nomic");
        s.ffmpeg = "/non/esiste/ffmpeg".into();
        s.toggle_recording();
        assert_eq!(s.rec, Rec::Idle);
        assert!(s.audio_error.as_deref().is_some_and(|e| e.contains("non riesco")));
        clean("nomic");
    }

    #[test]
    fn a_recorder_that_dies_alone_pauses_and_says_so() {
        let mut s = quiet("dies");
        let dir = temp("dies");
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("dies.sh");
        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        s.ffmpeg = bin.to_string_lossy().into_owned();
        s.toggle_recording();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(s.check_recorder());
        assert_eq!(s.rec, Rec::Paused);
        assert!(s.audio_error.is_some());
        assert!(s.session.segments.is_empty(), "niente audio, niente segmento");
        clean("dies");
    }

    #[test]
    fn the_counter_adds_the_running_segment_only_in_the_area_being_recorded() {
        let mut s = with_fake_mic("counter");
        s.session.segments.push(crate::model::AudioSegment { file: "audio/a0-1_000.ogg".into(), secs: 60.0, started: 1, transcribed: true });
        s.toggle_recording();
        s.now = s.seg_t0 + 30.0;
        assert_eq!(s.rec_secs() as u32, 90);
        assert_eq!(s.rec_area_name().as_deref(), Some("Area 1"));
        s.add_area();
        assert_eq!(s.rec_secs() as u32, 0, "l'area nuova parte da zero");
        s.pause_recording();
        clean("counter");
    }
}
