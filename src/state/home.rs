//! Il menu principale: l'elenco dei colloqui per cliente, da cui se ne apre uno o se ne comincia
//! uno nuovo. Aprire un altro colloquio salva quello in vista e ne carica uno da capo.

use std::path::PathBuf;

use rune::TextEdit;

use super::{Rec, State, unix_secs};
use crate::library::{self, Entry};
use crate::model;

pub struct Home {
    pub entries: Vec<Entry>,
    pub filter: TextEdit,
}

impl State {
    /// Il colloquio in vista non ha ancora niente: né cliente, né battute, né diagramma, né altro.
    pub fn is_blank(&self) -> bool {
        self.client.trim().is_empty()
            && self.project().areas.iter().all(|a| {
                a.turns.is_empty() && a.mermaid.trim().is_empty() && a.questions.is_empty() && a.notes.trim().is_empty() && a.attachments.is_empty() && a.segments.is_empty()
            })
    }

    /// Apre il menu principale. Il colloquio in vista si salva prima, così compare aggiornato.
    pub fn open_home(&mut self) {
        self.finish_rename();
        if self.sync_edits() {
            self.touch();
        }
        if self.dirty {
            self.save();
        }
        let filter = self.home.take().map_or_else(|| TextEdit::new(""), |h| h.filter);
        self.home = Some(Home { entries: library::scan(&self.library_root), filter });
    }

    /// Torna al colloquio in vista.
    pub fn close_home(&mut self) {
        self.home = None;
    }

    /// Il colloquio è quello in vista.
    pub fn is_current(&self, dir: &std::path::Path) -> bool {
        self.dir == dir
    }

    /// Apre un colloquio salvato (quello in vista: torna semplicemente lì).
    pub fn open_session(&mut self, dir: PathBuf) {
        if self.is_current(&dir) {
            self.close_home();
            return;
        }
        self.switch_to(dir);
    }

    /// Comincia un colloquio nuovo; se quello in vista è ancora vuoto, si usa quello.
    pub fn new_session(&mut self) {
        if self.is_blank() {
            self.close_home();
            return;
        }
        let mut dir = self.library_root.join(format!("sessione-{}", unix_secs()));
        let mut n = 2;
        while dir.exists() {
            dir = self.library_root.join(format!("sessione-{}-{n}", unix_secs()));
            n += 1;
        }
        self.switch_to(dir);
    }

    /// Lascia il colloquio in vista (fermando la registrazione e salvando) e carica `dir` da capo.
    /// I lavori ancora in corso per il colloquio lasciato (agente, trascrizione) finiscono nel
    /// vuoto: l'audio non trascritto resta salvato e riparte quando lo si riapre.
    fn switch_to(&mut self, dir: PathBuf) {
        self.finish_rename();
        self.sync_edits();
        if self.rec == Rec::Live {
            self.end_segment();
        }
        if !self.is_blank() || self.dir.join("session.json").exists() {
            self.save();
        }
        let mut next = State::open_with(dir, self.persist, self.library_root.clone());
        next.auto = self.auto;
        if let Some(w) = self.waker.clone() {
            next.setup(w);
        }
        *self = next;
    }

    /// Un colloquio: quello salvato in `dir`, se c'è, con le impostazioni dell'utente.
    pub(super) fn open_with(dir: PathBuf, persist: bool, library_root: PathBuf) -> State {
        let mut s = State::with_dir(dir, persist);
        s.library_root = library_root;
        if let Some(saved) = model::load(&s.dir) {
            s.adopt_project(saved);
        }
        if persist {
            s.config_dir = crate::settings::config_dir();
        }
        if let Some(cfg) = s.config_dir.clone() {
            s.settings = crate::settings::Settings::load(&cfg);
        }
        s.refresh_key();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Speaker;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rr-home-{name}-{}", std::process::id()))
    }

    fn state(name: &str, session: &str) -> State {
        State::open_with(root(name).join(session), false, root(name))
    }

    fn saved(name: &str, session: &str, client: &str, line: &str) -> PathBuf {
        let mut s = state(name, session);
        s.client = client.into();
        s.session.add_turn(Speaker::Cliente, line);
        model::save(&s.dir, &s.project()).unwrap();
        s.dir.clone()
    }

    #[test]
    fn the_menu_lists_saved_sessions_and_opening_one_loads_it() {
        let _ = std::fs::remove_dir_all(root("open"));
        let rossana = saved("open", "rossana", "Test", "Usiamo Excel per gli ordini");
        let mut s = state("open", "nuova");
        assert!(s.is_blank());
        s.open_home();
        assert_eq!(s.home.as_ref().unwrap().entries.len(), 1, "quella vuota non è salvata e non compare");
        s.open_session(rossana.clone());
        assert!(s.home.is_none());
        assert_eq!(s.dir, rossana);
        assert_eq!(s.client, "Test");
        assert_eq!(s.session.turns[0].text, "Usiamo Excel per gli ordini");
        assert!(!root("open").join("nuova").exists(), "lasciare un colloquio vuoto non lascia cartelle");
        let _ = std::fs::remove_dir_all(root("open"));
    }

    #[test]
    fn switching_saves_what_was_typed_in_the_session_left_behind() {
        let _ = std::fs::remove_dir_all(root("save"));
        let other = saved("save", "altro", "Bianchi", "Parliamo del sito");
        let mut s = state("save", "prima");
        s.persist = true;
        s.client = "Rossi".into();
        s.notes_edit.text = "nota scritta adesso".into();
        s.open_session(other);
        assert_eq!(s.client, "Bianchi");
        let back = model::load(&root("save").join("prima")).expect("salvata");
        assert_eq!(back.client, "Rossi");
        assert_eq!(back.areas[0].notes, "nota scritta adesso");
        let _ = std::fs::remove_dir_all(root("save"));
    }

    #[test]
    fn a_new_session_reuses_a_blank_one_and_otherwise_starts_fresh() {
        let _ = std::fs::remove_dir_all(root("new"));
        let mut s = state("new", "vuota");
        s.open_home();
        s.new_session();
        assert!(s.home.is_none());
        assert_eq!(s.dir, root("new").join("vuota"), "era vuota: si resta lì");
        s.session.add_turn(Speaker::Io, "Buongiorno");
        s.new_session();
        assert_ne!(s.dir, root("new").join("vuota"));
        assert!(s.dir.starts_with(root("new")));
        assert!(s.is_blank());
        let _ = std::fs::remove_dir_all(root("new"));
    }

    #[test]
    fn opening_another_session_stops_the_recording_and_keeps_its_audio() {
        let _ = std::fs::remove_dir_all(root("rec"));
        let other = saved("rec", "altro", "Bianchi", "ciao");
        let mut s = state("rec", "registrata");
        s.persist = true;
        s.ffmpeg = crate::audio::tests::fake_ffmpeg(&root("rec"));
        s.toggle_recording();
        std::thread::sleep(std::time::Duration::from_millis(300));
        s.open_session(other);
        assert_eq!(s.rec, Rec::Idle, "il colloquio nuovo non registra");
        let back = model::load(&root("rec").join("registrata")).expect("salvata con il suo audio");
        assert_eq!(back.areas[0].segments.len(), 1);
        let _ = std::fs::remove_dir_all(root("rec"));
    }
}
