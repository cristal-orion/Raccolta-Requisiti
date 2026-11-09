//! I dati di un colloquio: battute, note, allegati, domande dell'agente e diagramma.
//! Tutto si salva in `session.json` dentro la cartella della sessione.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Speaker {
    Io,
    Cliente,
}

impl Speaker {
    pub fn label(self) -> &'static str {
        match self {
            Speaker::Io => "Io",
            Speaker::Cliente => "Cliente",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub speaker: Speaker,
    pub text: String,
    /// La finestra di audio da cui viene la battuta; `None` se l'hai scritta tu.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
    Image,
    Document,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    /// Copia dentro la cartella della sessione.
    pub path: PathBuf,
    pub kind: AttachmentKind,
    /// Cosa ne ha capito l'agente, riportata nei giri successivi al posto del file.
    #[serde(default)]
    pub summary: Option<String>,
}

/// Dove si trova una domanda dell'agente.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QStatus {
    /// Finestrella aperta accanto al nodo.
    Open,
    /// Chiusa senza rispondere: resta un puntino giallo accanto al nodo.
    Parked,
    Answered,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Question {
    pub id: u32,
    /// Il nodo Mermaid su cui l'agente ha il dubbio, se ne ha uno.
    pub node: Option<String>,
    pub text: String,
    pub answer: Option<String>,
    pub status: QStatus,
    /// Giro dell'agente in cui è nata.
    pub round: u32,
}

/// Una modifica che il consulente chiede su un nodo del diagramma.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instruction {
    pub id: u32,
    pub node: String,
    pub text: String,
    /// L'agente l'ha già applicata al diagramma.
    pub applied: bool,
}

/// Durata di ogni pezzo di audio che ffmpeg chiude da solo mentre registra.
pub const CHUNK_SECS: u32 = 20;
/// Quanti pezzi stanno in una finestra di trascrizione (10 minuti): dentro una finestra le voci
/// sono coerenti perché Gemini le riconosce in un'unica richiesta.
pub const CHUNKS_PER_WINDOW: u32 = 30;

/// Un pezzo di registrazione di `CHUNK_SECS` secondi (l'ultimo di ogni ripresa è più corto).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioSegment {
    /// Relativo alla cartella della sessione, es. `audio/a0-1762690000_003.ogg`.
    pub file: String,
    pub secs: f32,
    /// Quando è iniziato, in secondi Unix.
    pub started: u64,
    /// Il testo di questo pezzo è già nella trascrizione.
    #[serde(default)]
    pub transcribed: bool,
}

/// `audio/a0-1762690000_003.ogg` → area 0, avvio 1762690000, pezzo 3 (senza `_NNN` è il pezzo 0).
pub fn chunk_parts(file: &str) -> Option<(u32, u64, u32)> {
    let name = file.rsplit('/').next()?;
    let stem = name.strip_prefix('a')?.strip_suffix(".ogg")?;
    let (area, rest) = stem.split_once('-')?;
    let (ts, idx) = rest.split_once('_').unwrap_or((rest, "0"));
    Some((area.parse().ok()?, ts.parse().ok()?, idx.parse().ok()?))
}

impl AudioSegment {
    /// La finestra di trascrizione a cui appartiene, es. `a0-1762690000w0`.
    pub fn window(&self) -> Option<String> {
        let (area, ts, idx) = chunk_parts(&self.file)?;
        Some(format!("a{area}-{ts}w{}", idx / CHUNKS_PER_WINDOW))
    }

    pub fn index(&self) -> u32 {
        chunk_parts(&self.file).map_or(0, |(_, _, i)| i)
    }
}

/// Un'area di lavoro: un argomento (CRM, automazione…) con il suo interlocutore, il suo
/// diagramma, le sue note, i suoi allegati, le sue domande e il suo audio.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub id: u32,
    pub name: String,
    /// Con chi si parla su questo argomento.
    pub interlocutor: String,
    pub segments: Vec<AudioSegment>,
    /// Le finestre in cui «Io» e «Cliente» sono stati scambiati a mano.
    pub voice_swaps: Vec<String>,
    pub turns: Vec<Turn>,
    pub notes: String,
    pub attachments: Vec<Attachment>,
    pub questions: Vec<Question>,
    pub summary: String,
    pub open_points: Vec<String>,
    pub mermaid: String,
    pub rounds: u32,
    #[serde(default)]
    pub next_question_id: u32,
    #[serde(default)]
    pub instructions: Vec<Instruction>,
}

impl Session {
    pub fn area(id: u32, name: &str) -> Session {
        Session { id, name: name.to_string(), ..Session::default() }
    }

    /// Come si chiama chi parla: «Io», oppure l'interlocutore dell'area (o «Cliente»).
    pub fn speaker_name(&self, who: Speaker) -> String {
        match who {
            Speaker::Io => "Io".to_string(),
            Speaker::Cliente if self.interlocutor.trim().is_empty() => "Cliente".to_string(),
            Speaker::Cliente => self.interlocutor.trim().to_string(),
        }
    }

    pub fn audio_secs(&self) -> f32 {
        self.segments.iter().map(|s| s.secs).sum()
    }

    /// Audio registrato che non è ancora diventato testo.
    pub fn pending_audio_secs(&self) -> f32 {
        self.segments.iter().filter(|s| !s.transcribed).map(|s| s.secs).sum()
    }

    /// Mette le battute di una finestra al posto di quelle che aveva (o in fondo, se è nuova).
    /// Torna false se il testo è identico e non c'è niente da cambiare.
    pub fn replace_source_turns(&mut self, source: &str, turns: Vec<Turn>) -> bool {
        let same = |a: &Turn, b: &Turn| a.speaker == b.speaker && a.text == b.text;
        let old: Vec<&Turn> = self.turns.iter().filter(|t| t.source.as_deref() == Some(source)).collect();
        if old.len() == turns.len() && old.iter().zip(&turns).all(|(a, b)| same(a, b)) {
            return false;
        }
        let at = self.turns.iter().position(|t| t.source.as_deref() == Some(source)).unwrap_or(self.turns.len());
        self.turns.retain(|t| t.source.as_deref() != Some(source));
        let at = at.min(self.turns.len());
        self.turns.splice(at..at, turns);
        true
    }

    /// Scambia «Io» e «Cliente» nelle battute di una finestra, e se ne ricorda per le prossime passate.
    pub fn swap_voices(&mut self, source: &str) {
        for t in self.turns.iter_mut().filter(|t| t.source.as_deref() == Some(source)) {
            t.speaker = if t.speaker == Speaker::Io { Speaker::Cliente } else { Speaker::Io };
        }
        if let Some(i) = self.voice_swaps.iter().position(|w| w == source) {
            self.voice_swaps.remove(i);
        } else {
            self.voice_swaps.push(source.to_string());
        }
    }

    pub fn add_turn(&mut self, speaker: Speaker, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            self.turns.push(Turn { speaker, text: text.to_string(), source: None });
        }
    }

    pub fn add_question(&mut self, node: Option<String>, text: String) -> u32 {
        let id = self.next_question_id;
        self.next_question_id += 1;
        self.questions.push(Question { id, node, text, answer: None, status: QStatus::Open, round: self.rounds });
        id
    }

    pub fn add_instruction(&mut self, node: &str, text: &str) -> Option<u32> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let id = self.instructions.iter().map(|i| i.id + 1).max().unwrap_or(0);
        self.instructions.push(Instruction { id, node: node.to_string(), text: text.to_string(), applied: false });
        Some(id)
    }

    pub fn pending_instructions(&self) -> impl Iterator<Item = &Instruction> {
        self.instructions.iter().filter(|i| !i.applied)
    }

    pub fn question_mut(&mut self, id: u32) -> Option<&mut Question> {
        self.questions.iter_mut().find(|q| q.id == id)
    }

    /// Chiude la finestrella: la domanda resta salvata e diventa un puntino.
    pub fn park(&mut self, id: u32) {
        if let Some(q) = self.question_mut(id) {
            if q.status == QStatus::Open {
                q.status = QStatus::Parked;
            }
        }
    }

    pub fn reopen(&mut self, id: u32) {
        if let Some(q) = self.question_mut(id) {
            if q.status == QStatus::Parked {
                q.status = QStatus::Open;
            }
        }
    }

    /// Registra la risposta; torna false se il testo è vuoto.
    pub fn answer(&mut self, id: u32, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        match self.question_mut(id) {
            Some(q) => {
                q.answer = Some(text.to_string());
                q.status = QStatus::Answered;
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub fn open_questions(&self) -> impl Iterator<Item = &Question> {
        self.questions.iter().filter(|q| q.status == QStatus::Open)
    }

    #[cfg(test)]
    pub fn parked_questions(&self) -> impl Iterator<Item = &Question> {
        self.questions.iter().filter(|q| q.status == QStatus::Parked)
    }

    pub fn pending_count(&self) -> usize {
        self.questions.iter().filter(|q| q.status != QStatus::Answered).count()
    }
}

/// `$XDG_DATA_HOME/raccolta-requisiti/sessioni` (o sotto `~/.local/share`).
pub fn sessions_root() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("raccolta-requisiti").join("sessioni")
}

/// Tutto il colloquio: il cliente e le sue aree di lavoro.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub client: String,
    /// L'area che era in vista.
    pub active: usize,
    pub next_area_id: u32,
    pub areas: Vec<Session>,
}

impl Project {
    pub fn single(name: &str) -> Project {
        Project { client: String::new(), active: 0, next_area_id: 1, areas: vec![Session::area(0, name)] }
    }
}

/// La sessione salvata più di recente (per `session.json`) sotto `root`, per riprenderla.
pub fn latest_session(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|e| Some((e.path(), std::fs::metadata(e.path().join("session.json")).ok()?.modified().ok()?)))
        .max_by_key(|(_, modified)| *modified)
        .map(|(dir, _)| dir)
}

pub fn save(dir: &Path, p: &Project) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("session.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(p).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, dir.join("session.json"))
}

/// Carica il progetto; un file della prima versione (una sola area, senza `areas`) diventa
/// un progetto con una sola area.
pub fn load(dir: &Path) -> Option<Project> {
    let bytes = std::fs::read(dir.join("session.json")).ok()?;
    let mut p = match serde_json::from_slice::<Project>(&bytes) {
        Ok(p) if !p.areas.is_empty() => p,
        _ => {
            let mut one: Session = serde_json::from_slice(&bytes).ok()?;
            if one.name.is_empty() {
                one.name = "Area 1".into();
            }
            Project { client: String::new(), active: 0, next_area_id: one.id + 1, areas: vec![one] }
        }
    };
    p.active = p.active.min(p.areas.len() - 1);
    p.next_area_id = p.next_area_id.max(p.areas.iter().map(|a| a.id + 1).max().unwrap_or(0));
    adopt_orphan_audio(dir, &mut p);
    Some(p)
}

/// Un'app chiusa durante la registrazione lascia pezzi di audio che nessuna area elenca: li
/// riassegna all'area giusta dal nome (`a<area>-<unix>_<pezzo>.ogg`), con la durata stimata dalla
/// dimensione (32 kbit/s), e restano da trascrivere.
fn adopt_orphan_audio(dir: &Path, p: &mut Project) {
    let Ok(entries) = std::fs::read_dir(dir.join("audio")) else { return };
    for e in entries.flatten() {
        let file = format!("audio/{}", e.file_name().to_string_lossy());
        let Some((area, ts, idx)) = chunk_parts(&file) else { continue };
        let len = e.metadata().map_or(0, |m| m.len());
        if len == 0 || p.areas.iter().any(|a| a.segments.iter().any(|s| s.file == file)) {
            continue;
        }
        if let Some(a) = p.areas.iter_mut().find(|a| a.id == area) {
            let secs = (len as f32 * 8.0 / 32_000.0).min(CHUNK_SECS as f32);
            a.segments.push(AudioSegment { file, secs, started: ts + (idx * CHUNK_SECS) as u64, transcribed: false });
            a.segments.sort_by_key(|s| s.started);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_question_stays_saved_as_a_dot_and_can_come_back() {
        let mut s = Session::default();
        let id = s.add_question(Some("n2".into()), "Chi stampa il foglio?".into());
        assert_eq!(s.open_questions().count(), 1);
        s.park(id);
        assert_eq!(s.open_questions().count(), 0);
        assert_eq!(s.parked_questions().count(), 1);
        s.reopen(id);
        assert_eq!(s.open_questions().count(), 1);
    }

    #[test]
    fn answering_needs_text_and_removes_the_question_from_the_board() {
        let mut s = Session::default();
        let id = s.add_question(None, "Quanti ordini al giorno?".into());
        assert!(!s.answer(id, "   "));
        assert!(s.answer(id, " circa 40 "));
        assert_eq!(s.questions[0].answer.as_deref(), Some("circa 40"));
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn an_empty_instruction_is_not_recorded_and_ids_do_not_repeat() {
        let mut s = Session::default();
        assert_eq!(s.add_instruction("n1", "  "), None);
        let a = s.add_instruction("n1", "Aggiungi un controllo").unwrap();
        let b = s.add_instruction("n2", "Rinomina in «Verifica»").unwrap();
        assert_ne!(a, b);
        assert_eq!(s.pending_instructions().count(), 2);
    }

    #[test]
    fn questions_survive_a_save_and_load() {
        let dir = std::env::temp_dir().join(format!("rr-test-{}", std::process::id()));
        let mut p = Project::single("CRM");
        let id = p.areas[0].add_question(Some("A".into()), "Perché?".into());
        p.areas[0].park(id);
        save(&dir, &p).unwrap();
        let back = load(&dir).unwrap();
        assert_eq!(back.areas[0].questions[0].status, QStatus::Parked);
        assert_eq!(back.areas[0].questions[0].node.as_deref(), Some("A"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_latest_session_is_the_one_saved_last_and_folders_without_a_save_do_not_count() {
        let root = std::env::temp_dir().join(format!("rr-latest-{}", std::process::id()));
        for name in ["vecchia", "recente", "vuota"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let write = |name: &str, secs_ago: u64| {
            let f = root.join(name).join("session.json");
            std::fs::write(&f, "{}").unwrap();
            std::fs::File::options().write(true).open(&f).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago)).unwrap();
        };
        write("vecchia", 5000);
        write("recente", 10);
        assert_eq!(latest_session(&root).unwrap().file_name().unwrap(), "recente");
        assert!(latest_session(&root.join("non-esiste")).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_from_the_first_version_opens_as_a_single_area() {
        let dir = std::env::temp_dir().join(format!("rr-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), r#"{"client":"Rossi","turns":[{"speaker":"Io","text":"Ciao"}],"notes":"n"}"#).unwrap();
        let p = load(&dir).unwrap();
        assert_eq!(p.areas.len(), 1);
        assert_eq!(p.areas[0].name, "Area 1");
        assert_eq!(p.areas[0].turns.len(), 1);
        assert_eq!(p.areas[0].notes, "n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn audio_left_by_a_closed_app_goes_back_to_its_area() {
        let dir = std::env::temp_dir().join(format!("rr-orphan-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        std::fs::write(dir.join("audio/a1-1762690100_000.ogg"), vec![0u8; 8000]).unwrap();
        std::fs::write(dir.join("audio/a1-1762690100_001.ogg"), vec![0u8; 4000]).unwrap();
        std::fs::write(dir.join("audio/a0-1762690000.ogg"), vec![0u8; 4000]).unwrap();
        std::fs::write(dir.join("audio/ffmpeg.log"), "x").unwrap();
        let mut p = Project::single("CRM");
        p.areas.push(Session::area(1, "Automazione"));
        p.areas[0].segments.push(AudioSegment { file: "audio/a0-1762690000.ogg".into(), secs: 1.0, started: 1762690000, transcribed: true });
        save(&dir, &p).unwrap();
        let back = load(&dir).unwrap();
        assert_eq!(back.areas[0].segments.len(), 1, "già elencato: non si duplica");
        assert_eq!(back.areas[1].segments.len(), 2);
        assert!((back.areas[1].segments[0].secs - 2.0).abs() < 0.01, "8000 byte a 32 kbit/s sono 2 s");
        assert_eq!(back.areas[1].segments[1].started, 1762690100 + 20, "il secondo pezzo parte 20 s dopo");
        assert!(!back.areas[1].segments[0].transcribed, "da trascrivere");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_interlocutor_names_the_client_side_of_the_conversation() {
        let mut a = Session::area(0, "CRM");
        assert_eq!(a.speaker_name(Speaker::Cliente), "Cliente");
        a.interlocutor = " Marco, IT ".into();
        assert_eq!(a.speaker_name(Speaker::Cliente), "Marco, IT");
        assert_eq!(a.speaker_name(Speaker::Io), "Io");
    }

    #[test]
    fn a_chunk_name_tells_its_area_its_start_and_its_window() {
        assert_eq!(chunk_parts("audio/a2-1762690000_031.ogg"), Some((2, 1762690000, 31)));
        assert_eq!(chunk_parts("audio/a0-5.ogg"), Some((0, 5, 0)), "il formato senza numero di pezzo");
        assert_eq!(chunk_parts("audio/ffmpeg.log"), None);
        let seg = |f: &str| AudioSegment { file: f.into(), secs: 20.0, started: 0, transcribed: false };
        assert_eq!(seg("audio/a2-1762690000_029.ogg").window().as_deref(), Some("a2-1762690000w0"));
        assert_eq!(seg("audio/a2-1762690000_030.ogg").window().as_deref(), Some("a2-1762690000w1"), "dopo 10 minuti si apre un'altra finestra");
    }

    fn turn(who: Speaker, text: &str, source: Option<&str>) -> Turn {
        Turn { speaker: who, text: text.into(), source: source.map(String::from) }
    }

    #[test]
    fn a_window_is_replaced_in_place_and_my_own_lines_stay_where_they_are() {
        let mut s = Session::area(0, "CRM");
        s.turns.push(turn(Speaker::Io, "scritta prima", None));
        s.turns.push(turn(Speaker::Cliente, "Gli ordini", Some("w0")));
        s.turns.push(turn(Speaker::Io, "scritta dopo", None));
        let changed = s.replace_source_turns("w0", vec![turn(Speaker::Cliente, "Gli ordini arrivano per email", Some("w0")), turn(Speaker::Io, "Chi li inserisce?", Some("w0"))]);
        assert!(changed);
        let texts: Vec<&str> = s.turns.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["scritta prima", "Gli ordini arrivano per email", "Chi li inserisce?", "scritta dopo"]);
    }

    #[test]
    fn replacing_a_window_with_the_same_text_changes_nothing() {
        let mut s = Session::area(0, "CRM");
        s.turns.push(turn(Speaker::Cliente, "Ciao", Some("w0")));
        assert!(!s.replace_source_turns("w0", vec![turn(Speaker::Cliente, "Ciao", Some("w0"))]));
        assert!(s.replace_source_turns("w1", vec![turn(Speaker::Io, "Nuova finestra", Some("w1"))]), "una finestra nuova va in fondo");
        assert_eq!(s.turns.last().unwrap().text, "Nuova finestra");
    }

    #[test]
    fn swapping_the_voices_flips_only_that_window_and_swapping_twice_restores_it() {
        let mut s = Session::area(0, "CRM");
        s.turns.push(turn(Speaker::Io, "a", Some("w0")));
        s.turns.push(turn(Speaker::Cliente, "b", Some("w0")));
        s.turns.push(turn(Speaker::Io, "c", Some("w1")));
        s.turns.push(turn(Speaker::Io, "mia", None));
        s.swap_voices("w0");
        assert_eq!(s.turns.iter().map(|t| t.speaker).collect::<Vec<_>>(), [Speaker::Cliente, Speaker::Io, Speaker::Io, Speaker::Io]);
        assert_eq!(s.voice_swaps, ["w0"]);
        s.swap_voices("w0");
        assert_eq!(s.turns[0].speaker, Speaker::Io);
        assert!(s.voice_swaps.is_empty());
    }
}
