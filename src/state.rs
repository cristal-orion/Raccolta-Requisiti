//! Lo stato dell'app e le azioni che lo cambiano. La vista (`ui`) legge soltanto.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use rune::{Image, ImageCache, Live, TextEdit, Tx, Waker};

use crate::agent::{self, Delivery, Reply};
use crate::audio::Recorder;
use crate::model::{self, Attachment, AttachmentKind, Project, QStatus, Session, Speaker};
use crate::settings::{KeySource, Settings};

mod areas;
mod camera;
mod home;
mod settings_form;
mod transcript;

pub use areas::Rec;
use areas::Parked;
pub use camera::{CANVAS_MARGIN, Zoom};
pub use home::Home;
pub use settings_form::{KeyCheck, Picker, SettingsForm};
use transcript::InFlight;

/// Secondi minimi tra la fine di un giro e l'inizio del successivo, in automatico.
const AUTO_GAP: f64 = 25.0;
/// Domande nuove per giro: oltre questa soglia l'agente sarebbe invadente.
const MAX_NEW_QUESTIONS: usize = 2;
/// Con almeno queste domande ancora senza risposta, un giro ne aggiunge al massimo una.
const BUSY_QUESTIONS: usize = 2;
/// Finestrelle aperte insieme sulla lavagna: le più vecchie si riducono a puntini.
const MAX_OPEN_QUESTIONS: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum AgentStatus {
    Idle,
    Running,
    Failed(String),
}

/// L'esportazione in PDF.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportStatus {
    Idle,
    Running,
    Done(PathBuf),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Notes,
    Files,
    Questions,
}

pub struct State {
    /// Il cliente: vale per tutte le aree.
    pub client: String,
    /// L'area in vista. Le altre stanno in `slots`, e si scambiano con questa cambiando area.
    pub session: Session,
    slots: Vec<Option<Parked>>,
    pub active: usize,
    next_area_id: u32,
    pub renaming: bool,
    pub name_edit: TextEdit,
    pub interlocutor_edit: TextEdit,
    /// L'area per cui l'agente sta lavorando adesso.
    pub running_area: Option<u32>,
    pub rec: Rec,
    recorder: Option<Recorder>,
    rec_area: u32,
    rec_base: String,
    rec_next_idx: u32,
    rec_harvested: f32,
    rec_started: u64,
    seg_t0: f64,
    last_tick: i64,
    pub audio_error: Option<String>,
    ffmpeg: String,
    /// La trascrizione in corso: una sola richiesta a Gemini alla volta.
    transcribing: Option<InFlight>,
    tx_out: Sender<crate::transcribe::Outcome>,
    rx_out: Receiver<crate::transcribe::Outcome>,
    tx_last_start: f64,
    tx_retry_at: f64,
    pub transcribe_error: Option<String>,
    gemini_key: Option<String>,
    /// Da dove viene la chiave di Gemini in uso.
    pub key_source: KeySource,
    /// Modelli, chiave e microfono; si cambiano dal pannello delle impostazioni.
    pub settings: Settings,
    /// Dove si salvano le impostazioni (nessuna cartella: anteprime e test).
    config_dir: Option<PathBuf>,
    /// Il pannello delle impostazioni aperto, con le scelte non ancora salvate.
    pub form: Option<SettingsForm>,
    /// Il menu principale con l'elenco dei colloqui, quando è aperto.
    pub home: Option<Home>,
    /// La cartella che contiene tutti i colloqui.
    library_root: PathBuf,
    tx_check: Sender<settings_form::CheckOutcome>,
    rx_check: Receiver<settings_form::CheckOutcome>,
    pub dir: PathBuf,
    pub speaker: Speaker,
    pub turn_edit: TextEdit,
    pub notes_edit: TextEdit,
    pub client_edit: TextEdit,
    pub answer_edits: HashMap<u32, TextEdit>,
    /// Le bozze di risposta scritte nella scheda Domande (a parte da quelle sulla lavagna: due
    /// campi non possono condividere lo stesso testo in modifica).
    pub side_answers: HashMap<u32, TextEdit>,
    pub tab: Tab,
    pub auto: bool,
    pub agent: AgentStatus,
    pub drop_over: bool,
    /// Perché il selettore dei file non si è aperto, se è successo.
    pub file_error: Option<String>,
    pub export: ExportStatus,
    tx_export: Sender<crate::export::Delivery>,
    rx_export: Receiver<crate::export::Delivery>,
    /// Anteprima per l'esportazione: mostra soltanto il diagramma, senza il resto dell'interfaccia.
    pub export_only: bool,
    pub zoom: Zoom,
    /// Dove sta l'angolo del diagramma nella lavagna quando lo zoom è manuale.
    pan: [f32; 2],
    /// La camera per il disegno: si aggiorna senza ricostruire la vista.
    pub live_pan: Live<[f32; 2]>,
    pub live_zoom: Live<f32>,
    pan_from: [f32; 2],
    dragged: bool,
    glide: Option<camera::Glide>,
    samples: camera::Samples,
    /// Misure lette dal layout: il diagramma a grandezza naturale e lo spazio per mostrarlo.
    diagram_size: [f32; 2],
    viewport_size: [f32; 2],
    /// Il pannello di destra (note, allegati, domande) è aperto o ridotto a una striscia.
    pub right_open: bool,
    /// Il nodo di cui si sta scrivendo una modifica.
    pub editing: Option<String>,
    pub node_edit: TextEdit,
    /// Le modifiche partite con il giro in corso, da dare per applicate quando risponde.
    pub in_flight: Vec<u32>,
    /// Far partire un giro appena l'agente è libero, senza aspettare l'intervallo automatico.
    run_asap: bool,
    pub thumbs: HashMap<PathBuf, Image>,
    images: Option<ImageCache>,
    waker: Option<Waker>,
    tx: Sender<Delivery>,
    rx: Receiver<Delivery>,
    /// C'è materiale che l'agente non ha ancora visto.
    pub fresh: bool,
    last_end: f64,
    now: f64,
    dirty: bool,
    /// Senza cartella non si scrive su disco (anteprime e test).
    persist: bool,
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl State {
    pub fn new() -> State {
        // `RR_SESSION` apre un colloquio, `RR_RESUME` riprende l'ultimo. Altrimenti si parte da
        // un colloquio nuovo con sopra il menu dei colloqui salvati, se ce ne sono.
        let root = model::sessions_root();
        let chosen = std::env::var_os("RR_SESSION").map(PathBuf::from).or_else(|| std::env::var_os("RR_RESUME").and_then(|_| model::latest_session(&root)));
        let dir = chosen.clone().unwrap_or_else(|| root.join(format!("sessione-{}", unix_secs())));
        let mut s = State::open_with(dir, true, root.clone());
        if chosen.is_none() && model::latest_session(&root).is_some() {
            s.open_home();
        }
        s
    }

    /// Rilegge quale chiave di Gemini usare (salvata o dall'ambiente).
    pub(super) fn refresh_key(&mut self) {
        let (key, from) = crate::settings::active_key(self.config_dir.as_deref());
        self.gemini_key = key;
        self.key_source = from;
    }

    pub fn with_dir(dir: PathBuf, persist: bool) -> State {
        let (tx, rx) = channel();
        let (tx_out, rx_out) = channel();
        let (tx_export, rx_export) = channel();
        let (tx_check, rx_check) = channel();
        let (gemini_key, key_source) = crate::settings::active_key(None);
        State {
            client: String::new(),
            session: Session::area(0, "Area 1"),
            slots: vec![None],
            active: 0,
            next_area_id: 1,
            renaming: false,
            name_edit: TextEdit::new(""),
            interlocutor_edit: TextEdit::new(""),
            running_area: None,
            rec: Rec::Idle,
            recorder: None,
            rec_area: 0,
            rec_base: String::new(),
            rec_next_idx: 0,
            rec_harvested: 0.0,
            rec_started: 0,
            seg_t0: 0.0,
            last_tick: -1,
            audio_error: None,
            ffmpeg: crate::audio::default_bin(),
            transcribing: None,
            tx_out,
            rx_out,
            tx_last_start: -1000.0,
            tx_retry_at: 0.0,
            transcribe_error: None,
            gemini_key,
            key_source,
            settings: Settings::default(),
            config_dir: None,
            form: None,
            home: None,
            library_root: model::sessions_root(),
            tx_check,
            rx_check,
            dir,
            speaker: Speaker::Cliente,
            turn_edit: TextEdit::new(""),
            notes_edit: TextEdit::new(""),
            client_edit: TextEdit::new(""),
            answer_edits: HashMap::new(),
            side_answers: HashMap::new(),
            tab: Tab::Notes,
            auto: true,
            agent: AgentStatus::Idle,
            drop_over: false,
            file_error: None,
            export: ExportStatus::Idle,
            tx_export,
            rx_export,
            export_only: false,
            zoom: Zoom::Fit,
            pan: [CANVAS_MARGIN, CANVAS_MARGIN],
            live_pan: Live::new([CANVAS_MARGIN, CANVAS_MARGIN]),
            live_zoom: Live::new(1.0),
            pan_from: [0.0, 0.0],
            dragged: false,
            glide: None,
            samples: Vec::new(),
            diagram_size: [0.0, 0.0],
            viewport_size: [0.0, 0.0],
            right_open: true,
            editing: None,
            node_edit: TextEdit::new(""),
            in_flight: Vec::new(),
            run_asap: false,
            thumbs: HashMap::new(),
            images: None,
            waker: None,
            tx,
            rx,
            fresh: false,
            last_end: -AUTO_GAP,
            now: 0.0,
            dirty: false,
            persist,
        }
    }

    pub fn setup(&mut self, waker: Waker) {
        self.images = Some(ImageCache::new(waker.clone()));
        self.waker = Some(waker);
        self.load_all_thumbs();
    }

    fn load_all_thumbs(&mut self) {
        let mut files: Vec<PathBuf> = Vec::new();
        let sessions = std::iter::once(&self.session).chain(self.slots.iter().flatten().map(|p| &p.session));
        for s in sessions {
            files.extend(s.attachments.iter().filter(|a| a.kind == AttachmentKind::Image).map(|a| a.path.clone()));
        }
        for p in files {
            self.load_thumb(&p);
        }
    }

    fn load_thumb(&mut self, path: &Path) {
        if self.thumbs.contains_key(path) {
            return;
        }
        if let Some(cache) = &self.images {
            self.thumbs.insert(path.to_path_buf(), cache.file(path, 64.0, 2.0));
        }
    }

    pub fn answer_edit(&mut self, id: u32) -> &mut TextEdit {
        self.answer_edits.entry(id).or_insert_with(|| TextEdit::new(""))
    }

    pub fn side_answer_edit(&mut self, id: u32) -> &mut TextEdit {
        self.side_answers.entry(id).or_insert_with(|| TextEdit::new(""))
    }

    /// Si sta scrivendo una risposta a questa domanda, sulla lavagna o nella scheda Domande.
    fn drafting(&self, id: u32) -> bool {
        let has = |m: &HashMap<u32, TextEdit>| m.get(&id).is_some_and(|e| !e.text.trim().is_empty());
        has(&self.answer_edits) || has(&self.side_answers)
    }

    fn touch(&mut self) {
        self.dirty = true;
    }

    // ---- battute ----

    pub fn submit_turn(&mut self) {
        let text = self.turn_edit.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.session.add_turn(self.speaker, &text);
        self.turn_edit = TextEdit::new("");
        self.turn_edit.focused = true;
        self.fresh = true;
        self.touch();
    }

    // ---- allegati ----

    pub fn add_files(&mut self, files: &[PathBuf]) {
        let folder = self.dir.join("allegati").join(self.session.id.to_string());
        if self.persist && std::fs::create_dir_all(&folder).is_err() {
            return;
        }
        for f in files {
            let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let kind = if ["png", "jpg", "jpeg", "gif", "webp", "bmp"].contains(&ext.as_str()) { AttachmentKind::Image } else { AttachmentKind::Document };
            let name = self.unique_name(&name);
            let dest = folder.join(&name);
            let path = if self.persist {
                if std::fs::copy(f, &dest).is_err() {
                    continue;
                }
                dest
            } else {
                f.clone()
            };
            self.session.attachments.push(Attachment { name, path: path.clone(), kind, summary: None });
            if kind == AttachmentKind::Image {
                self.load_thumb(&path);
            }
            self.fresh = true;
        }
        self.touch();
    }

    fn unique_name(&self, name: &str) -> String {
        let taken = |n: &str| self.session.attachments.iter().any(|a| a.name == n);
        if !taken(name) {
            return name.to_string();
        }
        let p = Path::new(name);
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        (2..).map(|i| format!("{stem}-{i}{ext}")).find(|n| !taken(n)).unwrap_or_else(|| name.to_string())
    }

    // ---- domande ----

    pub fn close_question(&mut self, id: u32) {
        self.session.park(id);
        self.touch();
    }

    pub fn reopen_question(&mut self, id: u32) {
        self.session.reopen(id);
        self.touch();
    }

    /// La risposta scritta nella finestrella sulla lavagna.
    pub fn send_answer(&mut self, id: u32) {
        let text = self.answer_edit(id).text.clone();
        self.answer_with(id, &text);
    }

    /// La risposta scritta nella scheda Domande: vale come quella sulla lavagna, e la domanda
    /// sparisce da lì (finestrella o puntino che fosse).
    pub fn send_side_answer(&mut self, id: u32) {
        let text = self.side_answer_edit(id).text.clone();
        self.answer_with(id, &text);
    }

    fn answer_with(&mut self, id: u32, text: &str) {
        if self.session.answer(id, text) {
            self.answer_edits.remove(&id);
            self.side_answers.remove(&id);
            self.fresh = true;
            self.touch();
        }
    }

    // ---- esportazione ----

    pub fn can_export(&self) -> bool {
        self.export != ExportStatus::Running && self.waker.is_some() && self.project().areas.iter().any(|a| !a.turns.is_empty() || !a.mermaid.trim().is_empty() || !a.questions.is_empty())
    }

    /// Prepara il verbale in PDF con tutte le aree: ci pensa GPT, in un thread.
    pub fn start_export(&mut self) {
        if !self.can_export() {
            return;
        }
        let Some(wake) = self.waker.clone() else { return };
        self.sync_edits();
        self.save();
        self.export = ExportStatus::Running;
        crate::export::spawn(self.project(), self.dir.clone(), self.settings.agent(), self.tx_export.clone(), wake);
    }

    /// Per l'anteprima del diagramma da esportare: legge il sorgente da `RR_EXPORT_MERMAID`.
    pub fn export_diagram() -> State {
        let mut s = State::with_dir(std::env::temp_dir().join("rr-export-diagram"), false);
        s.session.mermaid = std::env::var_os("RR_EXPORT_MERMAID").and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        s.export_only = true;
        s.zoom = Zoom::Manual(1.0);
        s
    }

    // ---- pannelli ----

    pub fn toggle_right(&mut self) {
        self.right_open = !self.right_open;
    }

    /// Apre il pannello di destra su una scheda (dalla striscia, cliccando i contatori).
    pub fn open_right(&mut self, tab: Tab) {
        self.right_open = true;
        self.tab = tab;
    }

    // ---- modifiche ai nodi ----

    /// Un clic sul nodo apre la finestrella di modifica; un secondo clic la chiude.
    pub fn toggle_edit(&mut self, node: &str) {
        if self.editing.as_deref() == Some(node) {
            self.cancel_edit();
        } else {
            self.editing = Some(node.to_string());
            self.node_edit = TextEdit::new("");
            self.node_edit.focused = true;
        }
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.node_edit = TextEdit::new("");
    }

    /// Registra la modifica e fa partire subito l'agente: è una richiesta esplicita.
    pub fn submit_edit(&mut self) {
        let Some(node) = self.editing.clone() else { return };
        let text = self.node_edit.text.clone();
        if self.session.add_instruction(&node, &text).is_some() {
            self.cancel_edit();
            self.fresh = true;
            self.run_asap = true;
            self.touch();
        }
    }

    // ---- agente ----

    pub fn can_run(&self) -> bool {
        self.agent != AgentStatus::Running && self.waker.is_some()
    }

    pub fn start_round(&mut self) {
        if !self.can_run() {
            return;
        }
        let Some(wake) = self.waker.clone() else { return };
        self.agent = AgentStatus::Running;
        self.running_area = Some(self.session.id);
        self.fresh = false;
        self.run_asap = false;
        self.in_flight = self.session.pending_instructions().map(|i| i.id).collect();
        agent::spawn(self.session.clone(), self.dir.join(".agente"), self.settings.agent(), self.tx.clone(), wake);
    }

    pub fn apply_reply(&mut self, r: Reply) -> Result<(), String> {
        if !rune_mermaid::supported(&r.mermaid) {
            return Err("l'agente ha prodotto un diagramma che non so disegnare".into());
        }
        self.session.rounds += 1;
        for i in self.session.instructions.iter_mut().filter(|i| self.in_flight.contains(&i.id)) {
            i.applied = true;
        }
        self.in_flight.clear();
        self.session.mermaid = r.mermaid;
        self.session.summary = r.summary;
        self.session.open_points = r.open_points;
        for a in r.attachments {
            if let Some(att) = self.session.attachments.iter_mut().find(|x| x.name == a.file) {
                att.summary = Some(a.summary);
            }
        }
        let mut added = Vec::new();
        // Se il consulente ha già domande in attesa, una nuova alla volta basta.
        let cap = if self.session.pending_count() >= BUSY_QUESTIONS { 1 } else { MAX_NEW_QUESTIONS };
        for q in r.questions {
            let text = q.text.trim().to_string();
            if text.is_empty() || added.len() >= cap || self.is_known_question(&text) {
                continue;
            }
            let node = q.node_id.filter(|n| has_node(&self.session.mermaid, n));
            let id = self.session.add_question(node, text);
            self.answer_edits.insert(id, TextEdit::new(""));
            added.push(id);
        }
        self.limit_open_questions(&added);
        self.touch();
        Ok(())
    }

    /// Tiene aperte al massimo `MAX_OPEN_QUESTIONS` finestrelle: le più vecchie diventano puntini
    /// (restano salvate). Non chiude una domanda a cui stai scrivendo la risposta, né una appena
    /// arrivata (`fresh`): in quei casi ne restano aperte di più.
    fn limit_open_questions(&mut self, fresh: &[u32]) {
        let mut open: Vec<u32> = self.session.questions.iter().filter(|q| q.status == QStatus::Open).map(|q| q.id).collect();
        while open.len() > MAX_OPEN_QUESTIONS {
            let Some(pos) = open.iter().position(|&id| !self.drafting(id) && !fresh.contains(&id)) else { break };
            let id = open.remove(pos);
            self.session.park(id);
        }
    }

    fn is_known_question(&self, text: &str) -> bool {
        let norm = |s: &str| s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
        let t = norm(text);
        self.session.questions.iter().any(|q| norm(&q.text) == t)
    }

    // ---- ciclo ----

    /// Una volta per frame. Ritorna true se qualcosa di visibile è cambiato.
    pub fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        self.now = now;
        let mut changed = false;

        while let Ok((area, done)) = self.rx.try_recv() {
            self.last_end = now;
            self.running_area = None;
            self.agent = match done.and_then(|r| self.deliver(area, r)) {
                Ok(()) => AgentStatus::Idle,
                Err(e) => AgentStatus::Failed(e),
            };
            changed = true;
        }

        while let Ok(done) = self.rx_export.try_recv() {
            self.export = match done {
                Ok(pdf) => {
                    open_file(&pdf);
                    ExportStatus::Done(pdf)
                }
                Err(e) => ExportStatus::Failed(e),
            };
            changed = true;
        }

        if self.sync_edits() {
            self.touch();
        }
        if self.client_edit.text != self.client {
            self.client = self.client_edit.text.clone();
            self.touch();
        }
        changed |= self.tick_recording(now, tx);
        changed |= self.pump_transcription(now, tx);
        changed |= self.step_glide(now, tx);
        changed |= self.collect_key_check();

        if self.run_asap && self.can_run() {
            self.start_round();
            changed = true;
        }

        if self.auto && self.fresh && self.can_run() {
            if now - self.last_end >= AUTO_GAP {
                self.start_round();
                changed = true;
            } else {
                tx.wake_at(self.last_end + AUTO_GAP);
            }
        }

        if self.dirty {
            self.save();
        }
        changed
    }

    pub fn save(&mut self) {
        self.dirty = false;
        if self.persist {
            let _ = model::save(&self.dir, &self.project());
        }
    }

    /// Per le anteprime: un colloquio di esempio con due aree. Nella prima c'è una domanda
    /// aperta, una in sospeso e una con risposta.
    pub fn demo() -> State {
        let mut s = State::with_dir(std::env::temp_dir().join("rr-demo"), false);
        let mut ses = Session::area(0, "CRM");
        ses.interlocutor = "Marco, responsabile vendite".into();
        ses.add_turn(Speaker::Io, "Buongiorno, mi racconta come gestite oggi gli ordini dei clienti?");
        ses.add_turn(Speaker::Cliente, "Arrivano per email e per telefono, poi la segretaria li inserisce a mano nel gestionale.");
        ses.add_turn(Speaker::Io, "E quando l'ordine è inserito, chi avvisa il magazzino?");
        ses.add_turn(Speaker::Cliente, "Nessuno in automatico: stampiamo un foglio e lo portiamo giù. Spesso si perdono.");
        ses.mermaid = "flowchart TD\n    n1[Ordine ricevuto] --> n2{Canale}\n    n2 -->|Email| n3[Segretaria inserisce nel gestionale]\n    n2 -->|Telefono| n3\n    n3 --> n4[Stampa foglio ordine]\n    n4 --> n5[Foglio portato al magazzino]\n    n5 --> n6[Preparazione ordine]".into();
        ses.summary = "Gli ordini arrivano via email o telefono e vengono inseriti a mano nel gestionale. Il magazzino è avvisato con un foglio stampato, che spesso si perde.".into();
        ses.open_points = vec!["Quale gestionale usano".into(), "Volume giornaliero di ordini".into()];
        ses.notes = "Gestionale: da chiedere. Possibile integrazione email → ordine.".into();
        ses.rounds = 1;
        ses.segments.push(model::AudioSegment { file: "audio/a0-1_000.ogg".into(), secs: 754.0, started: 1, transcribed: true });
        let a = ses.add_question(Some("n3".into()), "Gli ordini per telefono vengono registrati prima su carta o direttamente nel gestionale?".into());
        let b = ses.add_question(Some("n5".into()), "Chi porta il foglio al magazzino e con quale frequenza?".into());
        let c = ses.add_question(Some("n2".into()), "Ci sono altri canali oltre a email e telefono?".into());
        ses.park(b);
        ses.answer(c, "Qualche ordine arriva anche via WhatsApp");
        let _ = a;

        let mut auto = Session::area(1, "Automazione interna");
        auto.interlocutor = "Laura, amministrazione".into();
        auto.add_turn(Speaker::Io, "Passiamo alle attività interne: cosa fate a mano ogni settimana?");
        auto.add_turn(Speaker::Cliente, "Ogni lunedì copio i dati delle fatture dal gestionale in un foglio Excel per il controllo di gestione.");
        auto.turns.iter_mut().for_each(|t| t.source = Some("a1-2w0".into()));
        auto.mermaid = "flowchart TD\n    n1[Esportazione fatture dal gestionale] --> n2[Copia in Excel]\n    n2 --> n3[Controllo di gestione]".into();
        auto.summary = "Ogni lunedì le fatture vengono copiate a mano dal gestionale in Excel per il controllo di gestione.".into();
        auto.rounds = 1;
        auto.add_question(Some("n2".into()), "Quante fatture a settimana e con quali campi?".into());

        s.adopt_project(Project { client: "Rossi Meccanica".into(), active: 0, next_area_id: 2, areas: vec![ses, auto] });
        s.auto = false;
        s
    }
}

/// Apre un file con il programma predefinito del desktop (per mostrare il PDF appena fatto).
fn open_file(path: &Path) {
    if std::env::var_os("RR_NO_OPEN").is_some() {
        return;
    }
    let _ = std::process::Command::new("xdg-open").arg(path).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

/// Se `id` compare come nodo nel diagramma (parola intera).
pub fn has_node(mermaid: &str, id: &str) -> bool {
    !id.is_empty() && mermaid.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == id)
}

/// Gli id che compaiono nel Mermaid fuori dalle etichette: nodi, ma anche parole chiave come
/// `flowchart` o `end` (chi chiama controlla quali hanno davvero un nodo disegnato).
pub fn node_ids(mermaid: &str) -> Vec<String> {
    let mut clean = String::with_capacity(mermaid.len());
    let (mut depth, mut piped, mut quoted) = (0usize, false, false);
    for c in mermaid.chars() {
        match c {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => piped = !piped,
            _ if depth > 0 || piped => {}
            c => {
                clean.push(c);
                continue;
            }
        }
        clean.push(' ');
    }
    let mut seen = std::collections::HashSet::new();
    clean
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_'))
        .filter(|w| seen.insert(*w))
        .map(String::from)
        .collect()
}

/// L'etichetta con cui `id` è definito nel Mermaid (`n2{Canale}` → «Canale»), se si trova.
pub fn node_label(mermaid: &str, id: &str) -> Option<String> {
    for line in mermaid.lines() {
        let mut from = 0;
        while let Some(pos) = line[from..].find(id) {
            let start = from + pos;
            let end = start + id.len();
            from = end;
            let word = |c: char| c.is_alphanumeric() || c == '_';
            let before_ok = line[..start].chars().next_back().is_none_or(|c| !word(c));
            let after = &line[end..];
            if !before_ok || after.chars().next().is_none_or(word) {
                continue;
            }
            let body = after.trim_start_matches(['[', '(', '{', '>', '/', '\\']);
            if body.len() == after.len() {
                continue;
            }
            let label: String = body.chars().take_while(|c| !matches!(c, ']' | ')' | '}')).collect();
            let label = label.trim().trim_matches('"').replace("<br>", " ").replace("<br/>", " ");
            let label = label.trim().trim_end_matches(['/', '\\']).trim().to_string();
            if !label.is_empty() {
                return Some(label);
            }
        }
    }
    None
}

impl QStatus {
    pub fn label(self) -> &'static str {
        match self {
            QStatus::Open => "aperta sulla lavagna",
            QStatus::Parked => "in sospeso",
            QStatus::Answered => "risposta",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AttachmentSummary, NewQuestion};

    fn reply(questions: Vec<NewQuestion>) -> Reply {
        Reply {
            summary: "s".into(),
            mermaid: "flowchart TD\n  n1[Uno] --> n2[Due]".into(),
            open_points: vec![],
            questions,
            attachments: vec![],
        }
    }

    fn q(node: Option<&str>, text: &str) -> NewQuestion {
        NewQuestion { node_id: node.map(String::from), text: text.into() }
    }

    fn quiet() -> State {
        State::with_dir(std::env::temp_dir().join("rr-state-test"), false)
    }

    #[test]
    fn a_node_label_is_read_from_its_definition_in_any_shape() {
        let m = "flowchart TD\n  n1[Ordine ricevuto] --> n2{Canale}\n  n2 --> n3((Fine))\n  n3 --> n4[\"Con virgolette\"]\n  n4 --> n12[Dodici]\n  n5>Asimmetrico]";
        assert_eq!(node_label(m, "n1").as_deref(), Some("Ordine ricevuto"));
        assert_eq!(node_label(m, "n2").as_deref(), Some("Canale"));
        assert_eq!(node_label(m, "n3").as_deref(), Some("Fine"));
        assert_eq!(node_label(m, "n4").as_deref(), Some("Con virgolette"));
        assert_eq!(node_label(m, "n12").as_deref(), Some("Dodici"));
        assert_eq!(node_label(m, "n5").as_deref(), Some("Asimmetrico"));
        assert_eq!(node_label(m, "n9"), None);
    }

    #[test]
    fn node_ids_skip_the_words_inside_labels() {
        let m = "flowchart TD\n  n1[Ordine ricevuto] -->|Email o telefono| n2{Canale}\n  n2 --> n3((Fine))\n  n3 --> n4[\"Con virgolette\"]";
        let ids = node_ids(m);
        for want in ["n1", "n2", "n3", "n4"] {
            assert!(ids.contains(&want.to_string()), "manca {want}: {ids:?}");
        }
        for not in ["Ordine", "Email", "Canale", "Fine", "virgolette"] {
            assert!(!ids.contains(&not.to_string()), "{not} è solo un'etichetta");
        }
        assert_eq!(ids.iter().filter(|i| *i == "n2").count(), 1);
    }

    #[test]
    fn a_second_click_on_the_same_node_closes_its_editor() {
        let mut s = quiet();
        s.toggle_edit("n1");
        assert_eq!(s.editing.as_deref(), Some("n1"));
        s.toggle_edit("n2");
        assert_eq!(s.editing.as_deref(), Some("n2"), "un altro nodo sposta l'editor");
        s.toggle_edit("n2");
        assert_eq!(s.editing, None);
    }

    #[test]
    fn submitting_an_edit_records_it_and_asks_for_a_round_at_once() {
        let mut s = quiet();
        s.toggle_edit("n3");
        s.node_edit.text = "  Aggiungi un controllo qualità dopo questo passo ".into();
        s.submit_edit();
        assert_eq!(s.editing, None);
        assert_eq!(s.session.instructions.len(), 1);
        assert_eq!(s.session.instructions[0].node, "n3");
        assert!(s.run_asap && s.fresh);
    }

    #[test]
    fn an_empty_edit_keeps_the_editor_open_and_records_nothing() {
        let mut s = quiet();
        s.toggle_edit("n3");
        s.node_edit.text = "   ".into();
        s.submit_edit();
        assert_eq!(s.editing.as_deref(), Some("n3"));
        assert!(s.session.instructions.is_empty());
        assert!(!s.run_asap);
    }

    #[test]
    fn only_the_edits_sent_with_a_round_count_as_applied_when_it_answers() {
        let mut s = quiet();
        let sent = s.session.add_instruction("n1", "Rinomina in «Ricezione»").unwrap();
        s.in_flight = vec![sent];
        let later = s.session.add_instruction("n2", "Elimina questo passo").unwrap();
        s.apply_reply(reply(vec![])).unwrap();
        let applied = |id| s.session.instructions.iter().find(|i| i.id == id).unwrap().applied;
        assert!(applied(sent));
        assert!(!applied(later), "scritta dopo la partenza: resta per il giro successivo");
    }

    #[test]
    fn a_failed_round_leaves_the_edits_pending_to_be_retried() {
        let mut s = quiet();
        s.session.add_instruction("n1", "Dividi in due").unwrap();
        let mut bad = reply(vec![]);
        bad.mermaid = "non mermaid".into();
        assert!(s.apply_reply(bad).is_err());
        assert_eq!(s.session.pending_instructions().count(), 1);
    }

    #[test]
    fn the_side_panel_collapses_and_a_counter_reopens_it_on_the_right_tab() {
        let mut s = quiet();
        assert!(s.right_open);
        s.toggle_right();
        assert!(!s.right_open);
        s.open_right(Tab::Questions);
        assert!(s.right_open);
        assert_eq!(s.tab, Tab::Questions);
    }

    fn open_count(s: &State) -> usize {
        s.session.questions.iter().filter(|q| q.status == QStatus::Open).count()
    }

    #[test]
    fn only_the_two_newest_questions_stay_open_and_the_older_ones_become_dots() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Prima?"), q(Some("n2"), "Seconda?")])).unwrap();
        assert_eq!(open_count(&s), 2);
        s.apply_reply(reply(vec![q(Some("n1"), "Terza?")])).unwrap();
        s.apply_reply(reply(vec![q(Some("n2"), "Quarta?")])).unwrap();
        assert_eq!(open_count(&s), 2);
        let status = |text: &str| s.session.questions.iter().find(|x| x.text == text).unwrap().status;
        assert_eq!(status("Prima?"), QStatus::Parked);
        assert_eq!(status("Seconda?"), QStatus::Parked);
        assert_eq!(status("Terza?"), QStatus::Open);
        assert_eq!(status("Quarta?"), QStatus::Open);
        assert_eq!(s.session.questions.len(), 4, "nessuna domanda si perde");
    }

    #[test]
    fn a_question_being_answered_is_not_closed_by_a_newer_one() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Prima?"), q(Some("n2"), "Seconda?")])).unwrap();
        let first = s.session.questions[0].id;
        s.answer_edit(first).text = "La segretaria, ma anche".into();
        s.apply_reply(reply(vec![q(Some("n1"), "Terza?"), q(Some("n2"), "Quarta?")])).unwrap();
        let status = |text: &str| s.session.questions.iter().find(|x| x.text == text).unwrap().status;
        assert_eq!(status("Prima?"), QStatus::Open, "ha una bozza di risposta");
        assert_eq!(status("Seconda?"), QStatus::Parked, "al suo posto si chiude la successiva");
    }

    #[test]
    fn answered_questions_do_not_count_as_open() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Prima?"), q(Some("n2"), "Seconda?")])).unwrap();
        let first = s.session.questions[0].id;
        s.answer_edit(first).text = "Sì".into();
        s.send_answer(first);
        s.apply_reply(reply(vec![q(Some("n1"), "Terza?")])).unwrap();
        assert_eq!(open_count(&s), 2, "Seconda e Terza");
        assert_eq!(s.session.questions.iter().filter(|x| x.status == QStatus::Parked).count(), 0);
    }

    #[test]
    fn answering_from_the_questions_tab_closes_the_question_on_the_board_too() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Chi stampa?"), q(Some("n2"), "Quando?")])).unwrap();
        let (open, parked) = (s.session.questions[0].id, s.session.questions[1].id);
        s.close_question(parked);
        s.fresh = false;
        s.answer_edit(open).text = "bozza sulla lavagna".into();
        s.side_answer_edit(open).text = "  La segretaria ".into();
        s.send_side_answer(open);
        s.side_answer_edit(parked).text = "Ogni mattina".into();
        s.send_side_answer(parked);
        let status = |id: u32| s.session.questions.iter().find(|x| x.id == id).unwrap().status;
        assert_eq!(status(open), QStatus::Answered, "la finestrella sulla lavagna sparisce");
        assert_eq!(status(parked), QStatus::Answered, "e anche il puntino");
        assert_eq!(s.session.questions[0].answer.as_deref(), Some("La segretaria"));
        assert!(s.answer_edits.get(&open).is_none() && s.side_answers.get(&open).is_none(), "nessuna bozza resta in giro");
        assert!(s.fresh, "l'agente riceve la risposta");
        s.side_answer_edit(open).text = "  ".into();
        s.send_side_answer(open);
        assert_eq!(s.session.questions[0].answer.as_deref(), Some("La segretaria"), "una risposta vuota non cambia niente");
    }

    #[test]
    fn a_draft_in_the_questions_tab_keeps_its_bubble_open() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Prima?"), q(Some("n2"), "Seconda?")])).unwrap();
        let first = s.session.questions[0].id;
        s.side_answer_edit(first).text = "sto scrivendo".into();
        s.apply_reply(reply(vec![q(Some("n1"), "Terza?")])).unwrap();
        let status = |text: &str| s.session.questions.iter().find(|x| x.text == text).unwrap().status;
        assert_eq!(status("Prima?"), QStatus::Open);
        assert_eq!(status("Seconda?"), QStatus::Parked);
    }

    #[test]
    fn a_node_id_must_be_a_whole_word_of_the_diagram() {
        assert!(has_node("flowchart TD\n n1 --> n12", "n1"));
        assert!(!has_node("flowchart TD\n n12 --> n13", "n1"));
        assert!(!has_node("flowchart TD\n n1 --> n2", ""));
    }

    #[test]
    fn a_reply_adds_at_most_two_questions_and_drops_repeats() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Chi?"), q(Some("n2"), "Quando?"), q(None, "Perché?")])).unwrap();
        assert_eq!(s.session.questions.len(), 2);
        s.apply_reply(reply(vec![q(Some("n1"), "  chi?  "), q(None, "Dove?")])).unwrap();
        assert_eq!(s.session.questions.len(), 3, "«chi?» è già stata fatta");
        assert_eq!(s.session.rounds, 2);
    }

    #[test]
    fn with_questions_still_waiting_a_round_adds_only_one() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Chi?"), q(Some("n2"), "Quando?")])).unwrap();
        s.apply_reply(reply(vec![q(Some("n1"), "Con che cosa?"), q(Some("n2"), "Quanti al giorno?")])).unwrap();
        assert_eq!(s.session.questions.len(), 3, "due in attesa: ne entra una sola");
        for id in s.session.questions.iter().map(|x| x.id).collect::<Vec<_>>() {
            s.answer_edit(id).text = "risposta".into();
            s.send_answer(id);
        }
        s.apply_reply(reply(vec![q(Some("n1"), "Chi controlla?"), q(Some("n2"), "Dove si archivia?")])).unwrap();
        assert_eq!(s.session.questions.len(), 5, "tutte risposte: di nuovo fino a due");
    }

    #[test]
    fn a_question_on_a_node_that_does_not_exist_loses_its_anchor_but_is_kept() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("zz9"), "Dov'è?")])).unwrap();
        assert_eq!(s.session.questions[0].node, None);
    }

    #[test]
    fn an_undrawable_diagram_is_refused_and_changes_nothing() {
        let mut s = quiet();
        let mut r = reply(vec![q(None, "Boh?")]);
        r.mermaid = "questo non è mermaid".into();
        assert!(s.apply_reply(r).is_err());
        assert!(s.session.questions.is_empty());
        assert_eq!(s.session.rounds, 0);
    }

    #[test]
    fn attachment_summaries_land_on_the_right_file() {
        let mut s = quiet();
        s.session.attachments.push(Attachment { name: "a.png".into(), path: "a.png".into(), kind: AttachmentKind::Image, summary: None });
        let mut r = reply(vec![]);
        r.attachments = vec![AttachmentSummary { file: "a.png".into(), summary: "un foglio ordine".into() }];
        s.apply_reply(r).unwrap();
        assert_eq!(s.session.attachments[0].summary.as_deref(), Some("un foglio ordine"));
    }

    #[test]
    fn answering_marks_the_session_as_having_news_for_the_agent() {
        let mut s = quiet();
        s.apply_reply(reply(vec![q(Some("n1"), "Chi?")])).unwrap();
        s.fresh = false;
        let id = s.session.questions[0].id;
        s.answer_edit(id).text = "La segretaria".into();
        s.send_answer(id);
        assert!(s.fresh);
        assert_eq!(s.session.questions[0].status, QStatus::Answered);
    }

    #[test]
    fn two_files_with_the_same_name_do_not_overwrite_each_other() {
        let mut s = quiet();
        s.add_files(&["/x/foto.jpg".into(), "/y/foto.jpg".into()]);
        let names: Vec<_> = s.session.attachments.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, ["foto.jpg", "foto-2.jpg"]);
    }
}
