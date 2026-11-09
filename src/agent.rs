//! L'agente: a ogni giro manda a `codex exec` (il modello scelto nelle impostazioni) tutto quello
//! che sa del colloquio e riceve riassunto, diagramma Mermaid aggiornato e, se ha dubbi che
//! contano, domande ancorate ai nodi del diagramma.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::model::{Attachment, AttachmentKind, QStatus, Session};
use crate::settings::AgentConfig;

const TIMEOUT: Duration = Duration::from_secs(150);
/// Quanto testo di un documento entra nel prompt.
const DOC_LIMIT: usize = 20_000;

const SCHEMA: &str = r#"{
 "type":"object","additionalProperties":false,
 "required":["summary","mermaid","open_points","questions","attachments"],
 "properties":{
  "summary":{"type":"string"},
  "mermaid":{"type":"string"},
  "open_points":{"type":"array","items":{"type":"string"}},
  "questions":{"type":"array","items":{"type":"object","additionalProperties":false,
    "required":["node_id","text"],
    "properties":{"node_id":{"type":["string","null"]},"text":{"type":"string"}}}},
  "attachments":{"type":"array","items":{"type":"object","additionalProperties":false,
    "required":["file","summary"],
    "properties":{"file":{"type":"string"},"summary":{"type":"string"}}}}
 }}"#;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Reply {
    pub summary: String,
    pub mermaid: String,
    pub open_points: Vec<String>,
    pub questions: Vec<NewQuestion>,
    pub attachments: Vec<AttachmentSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct NewQuestion {
    pub node_id: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AttachmentSummary {
    pub file: String,
    pub summary: String,
}

pub fn parse_reply(json: &str) -> Result<Reply, String> {
    serde_json::from_str(json).map_err(|e| format!("risposta dell'agente non valida: {e}"))
}

/// Gli allegati che l'agente non ha ancora visto: li riceve una volta sola.
pub fn new_attachments(s: &Session) -> Vec<&Attachment> {
    s.attachments.iter().filter(|a| a.summary.is_none()).collect()
}

fn doc_text(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let raw = match ext.as_str() {
        "txt" | "md" | "csv" | "json" | "xml" | "html" | "log" => std::fs::read_to_string(path).ok()?,
        "pdf" => {
            let out = Command::new("pdftotext").arg("-layout").arg(path).arg("-").output().ok()?;
            if !out.status.success() {
                return None;
            }
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
        _ => return None,
    };
    Some(raw.chars().take(DOC_LIMIT).collect())
}

pub fn build_prompt(s: &Session) -> String {
    let mut p = String::new();
    p.push_str(
        "Sei l'assistente di un consulente che raccoglie requisiti (CRM, automazioni, siti web) \
durante un colloquio con un cliente. Ricevi tutto quello che è stato detto e scritto finora. \
Non eseguire comandi e non leggere file: lavora solo sul testo qui sotto.\n\n\
Restituisci:\n\
- summary: riassunto breve in italiano di ciò che hai capito finora.\n\
- mermaid: il processo descritto dal cliente come flowchart Mermaid (`flowchart TD` o `LR`), solo codice, \
senza ```. Usa id di nodo semplici e STABILI (n1, n2, ...): se il diagramma precedente esiste, mantieni gli \
stessi id per gli stessi passi, aggiungi nodi nuovi con id nuovi e non rinumerare mai. Etichette brevi in \
italiano. Se non c'è ancora niente da disegnare, un diagramma con un solo nodo.\n\
- open_points: punti ancora non chiari, come elenco breve.\n\
- questions: sei i secondi occhi del consulente, ma non devi interromperlo di continuo. Fai una domanda solo \
per un dubbio che conta per progettare la soluzione e che conviene chiarire adesso, prima che il cliente cambi \
argomento: in un passo manca o è ambiguo chi lo fa, con quale strumento, quanto spesso o con quali volumi, \
oppure il cliente dice \"dipende\" o \"a volte\" senza dire cosa succede. Lascia perdere i dettagli minori e \
quelli che emergeranno da soli continuando il racconto. Di norma al massimo una domanda per giro; due solo se \
sono entrambe importanti e su passi diversi; nessuna se non c'è un dubbio che pesa. Se ci sono già domande \
senza risposta, al massimo una nuova e solo se conta più di quelle in attesa. Ogni domanda va ancorata con \
node_id al nodo del diagramma a cui si riferisce (null solo se riguarda il colloquio in generale), è breve e \
concreta, si risponde in una frase. Non ripetere domande già fatte, nemmeno riformulate, e non chiedere ciò \
che risposte o trascrizione già dicono.\n\
- Se più sotto ci sono \"Modifiche richieste dal consulente\", applicale al diagramma con precedenza assoluta: \
rinomina, aggiungi passi con id nuovi, sposta, collega, elimina, come chiede. Gli altri id restano com'erano. \
Se la modifica è anche un fatto nuovo sul processo, tienine conto nel riassunto. Non fare domande su una modifica.\n\
- attachments: per ogni allegato NUOVO elencato sotto, una sintesi di cosa contiene e perché conta \
(file = nome esatto). Vuoto se non ce ne sono.\n\n",
    );

    p.push_str("## Area di lavoro\n");
    p.push_str(&format!("Argomento: {}\n", if s.name.trim().is_empty() { "non indicato" } else { s.name.trim() }));
    p.push_str(&format!("Interlocutore: {}\n", if s.interlocutor.trim().is_empty() { "non indicato" } else { s.interlocutor.trim() }));
    p.push_str("Il diagramma riguarda solo questo argomento: non mescolarlo con altri.\n\n");

    p.push_str("## Diagramma attuale\n");
    if s.mermaid.trim().is_empty() {
        p.push_str("(nessuno)\n\n");
    } else {
        p.push_str(&s.mermaid);
        p.push_str("\n\n");
    }

    p.push_str("## Trascrizione\n");
    if s.turns.is_empty() {
        p.push_str("(vuota)\n");
    }
    for t in &s.turns {
        p.push_str(&format!("[{}] {}\n", s.speaker_name(t.speaker), t.text));
    }

    if !s.notes.trim().is_empty() {
        p.push_str("\n## Note del consulente\n");
        p.push_str(s.notes.trim());
        p.push('\n');
    }

    let known: Vec<&Attachment> = s.attachments.iter().filter(|a| a.summary.is_some()).collect();
    if !known.is_empty() {
        p.push_str("\n## Allegati già esaminati\n");
        for a in known {
            p.push_str(&format!("- {}: {}\n", a.name, a.summary.as_deref().unwrap_or("")));
        }
    }

    let fresh = new_attachments(s);
    if !fresh.is_empty() {
        p.push_str("\n## Allegati NUOVI\n");
        for a in fresh {
            match a.kind {
                AttachmentKind::Image => p.push_str(&format!("- {} (immagine, allegata a questo messaggio)\n", a.name)),
                AttachmentKind::Document => match doc_text(&a.path) {
                    Some(t) => p.push_str(&format!("- {} (documento), testo:\n{}\n", a.name, t)),
                    None => p.push_str(&format!("- {} (documento di cui non posso leggere il contenuto)\n", a.name)),
                },
            }
        }
    }

    let pending: Vec<_> = s.pending_instructions().collect();
    if !pending.is_empty() {
        p.push_str("\n## Modifiche richieste dal consulente (da applicare al diagramma)\n");
        for i in pending {
            p.push_str(&format!("- nodo {}: {}\n", i.node, i.text));
        }
    }

    if !s.questions.is_empty() {
        p.push_str("\n## Domande già fatte al consulente\n");
        for q in &s.questions {
            let state = match (q.status, q.answer.as_deref()) {
                (QStatus::Answered, Some(a)) => format!("risposta: {a}"),
                _ => "senza risposta per ora".to_string(),
            };
            p.push_str(&format!("- [{}] {} ({})\n", q.node.as_deref().unwrap_or("generale"), q.text, state));
        }
    }
    p
}

pub fn codex_bin() -> String {
    std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".into())
}

/// Chiede un testo a `codex exec` con il modello di `cfg` e torna l'ultimo messaggio. Il modello non
/// esegue comandi (sandbox in sola lettura) e non vede la configurazione né le regole dell'utente.
/// `schema` impone la forma JSON della risposta; `images` si allegano alla richiesta; `effort`
/// sostituisce il ragionamento di `cfg` (l'esportazione ne chiede di più).
pub fn exec_text(cfg: &AgentConfig, prompt: &str, work_dir: &Path, schema: Option<&str>, images: &[PathBuf], effort: &str, timeout: Duration) -> Result<String, String> {
    std::fs::create_dir_all(work_dir).map_err(|e| e.to_string())?;
    let out = work_dir.join("risposta.txt");
    let _ = std::fs::remove_file(&out);

    let mut cmd = Command::new(&cfg.bin);
    cmd.args(["exec", "-m", &cfg.model, "--ignore-user-config", "--ignore-rules"]);
    if cfg.fast {
        cmd.args(["-c", "service_tier=priority"]);
    }
    cmd.arg("-c")
        .arg(format!("model_reasoning_effort={effort}"))
        .args(["-s", "read-only", "--skip-git-repo-check", "--ephemeral"]);
    if let Some(schema) = schema {
        let path = work_dir.join("schema.json");
        std::fs::write(&path, schema).map_err(|e| e.to_string())?;
        cmd.arg("--output-schema").arg(path);
    }
    cmd.arg("-o").arg(&out).current_dir(work_dir);
    for image in images {
        cmd.arg("-i").arg(image);
    }
    cmd.arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(work_dir.join("codex.log")).map_err(|e| e.to_string())?));

    let mut child = cmd.spawn().map_err(|e| format!("non riesco a lanciare codex ({}): {e}", cfg.bin))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    }
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => break,
            Some(status) => return Err(format!("codex è uscito con errore ({status}); vedi codex.log")),
            None if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("l'agente non ha risposto in tempo".into());
            }
            None => std::thread::sleep(Duration::from_millis(150)),
        }
    }
    std::fs::read_to_string(&out).map_err(|e| format!("nessuna risposta da codex: {e}"))
}

/// Un giro dell'agente, bloccante: da eseguire in un thread.
pub fn run(s: &Session, work_dir: &Path, cfg: &AgentConfig) -> Result<Reply, String> {
    let images: Vec<PathBuf> = new_attachments(s).into_iter().filter(|a| a.kind == AttachmentKind::Image).map(|a| a.path.clone()).collect();
    parse_reply(&exec_text(cfg, &build_prompt(s), work_dir, Some(SCHEMA), &images, &cfg.effort, TIMEOUT)?)
}

/// L'esito porta l'id dell'area per cui è stato chiesto: può arrivare quando se n'è aperta un'altra.
pub type Delivery = (u32, Result<Reply, String>);

pub fn spawn(s: Session, work_dir: PathBuf, cfg: AgentConfig, tx: Sender<Delivery>, wake: rune::Waker) {
    std::thread::spawn(move || {
        let area = s.id;
        let _ = tx.send((area, run(&s, &work_dir, &cfg)));
        wake();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Speaker;

    #[test]
    fn the_schema_is_valid_json() {
        let v: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(v["required"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn a_reply_parses_with_a_question_on_a_node_and_a_general_one() {
        let r = parse_reply(
            r#"{"summary":"s","mermaid":"flowchart TD\n n1-->n2","open_points":["x"],
            "questions":[{"node_id":"n2","text":"Chi stampa?"},{"node_id":null,"text":"Budget?"}],
            "attachments":[{"file":"a.png","summary":"un foglio"}]}"#,
        )
        .unwrap();
        assert_eq!(r.questions[0].node_id.as_deref(), Some("n2"));
        assert_eq!(r.questions[1].node_id, None);
        assert_eq!(r.attachments[0].file, "a.png");
    }

    #[test]
    fn a_broken_reply_is_an_error_not_a_panic() {
        assert!(parse_reply("{ non json").is_err());
        assert!(parse_reply(r#"{"summary":"s"}"#).is_err());
    }

    #[test]
    fn the_prompt_carries_answers_and_asks_not_to_repeat() {
        let mut s = Session::default();
        s.add_turn(Speaker::Cliente, "Gli ordini arrivano per email.");
        let id = s.add_question(Some("n1".into()), "Anche per telefono?".into());
        s.answer(id, "Sì, circa metà");
        s.add_question(Some("n2".into()), "Chi li inserisce?".into());
        let p = build_prompt(&s);
        assert!(p.contains("[Cliente] Gli ordini arrivano per email."));
        assert!(p.contains("risposta: Sì, circa metà"));
        assert!(p.contains("[n2] Chi li inserisce? (senza risposta per ora)"));
        assert!(p.contains("(nessuno)"), "senza diagramma lo dice");
    }

    #[test]
    fn the_prompt_names_the_area_and_the_interlocutor() {
        let mut s = Session::area(1, "Automazione interna");
        s.interlocutor = "Marco, responsabile IT".into();
        s.add_turn(Speaker::Cliente, "Usiamo ancora fogli Excel.");
        let p = build_prompt(&s);
        assert!(p.contains("Argomento: Automazione interna"));
        assert!(p.contains("Interlocutore: Marco, responsabile IT"));
        assert!(p.contains("[Marco, responsabile IT] Usiamo ancora fogli Excel."));
        let anon = build_prompt(&Session::default());
        assert!(anon.contains("Argomento: non indicato"));
    }

    #[test]
    fn only_pending_instructions_reach_the_agent() {
        let mut s = Session::default();
        let a = s.add_instruction("n2", "Dividi in due passi").unwrap();
        s.add_instruction("n3", "Aggiungi la firma del cliente").unwrap();
        s.instructions.iter_mut().find(|i| i.id == a).unwrap().applied = true;
        let p = build_prompt(&s);
        assert!(p.contains("Modifiche richieste dal consulente"));
        assert!(p.contains("nodo n3: Aggiungi la firma del cliente"));
        assert!(!p.contains("Dividi in due passi"), "quella già applicata non si ripete");
    }

    #[test]
    fn an_attachment_is_new_until_the_agent_summarises_it() {
        let mut s = Session::default();
        s.attachments.push(Attachment { name: "foto.jpg".into(), path: "foto.jpg".into(), kind: AttachmentKind::Image, summary: None });
        s.attachments.push(Attachment {
            name: "listino.txt".into(),
            path: "listino.txt".into(),
            kind: AttachmentKind::Document,
            summary: Some("prezzi 2026".into()),
        });
        assert_eq!(new_attachments(&s).len(), 1);
        let p = build_prompt(&s);
        assert!(p.contains("foto.jpg (immagine"));
        assert!(p.contains("listino.txt: prezzi 2026"));
    }

    #[test]
    fn the_prompt_asks_for_few_questions() {
        let p = build_prompt(&Session::default());
        assert!(p.contains("Di norma al massimo una domanda per giro"));
        assert!(!p.contains("sii proattivo"));
    }

    /// Un finto codex che salva i suoi argomenti e risponde con un JSON valido.
    fn fake_codex(dir: &Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("fake-codex.sh");
        let reply = r#"{"summary":"s","mermaid":"flowchart TD\n n1[Uno]","open_points":[],"questions":[],"attachments":[]}"#;
        std::fs::write(&path, format!("#!/bin/sh\necho \"$@\" > \"{}/args.txt\"\ncat > /dev/null\nout=\"\"\nwhile [ $# -gt 0 ]; do [ \"$1\" = \"-o\" ] && out=\"$2\"; shift; done\nprintf '%s' '{reply}' > \"$out\"\n", dir.display())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn the_model_the_effort_and_the_fast_tier_come_from_the_settings() {
        let dir = std::env::temp_dir().join(format!("rr-agent-cfg-{}", std::process::id()));
        let mut cfg = AgentConfig { bin: fake_codex(&dir), model: "gpt-6-luna".into(), effort: "medium".into(), fast: true };
        run(&Session::default(), &dir.join("w"), &cfg).unwrap();
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        assert!(args.contains("-m gpt-6-luna") && args.contains("service_tier=priority") && args.contains("model_reasoning_effort=medium"), "{args}");
        cfg.fast = false;
        run(&Session::default(), &dir.join("w"), &cfg).unwrap();
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        assert!(!args.contains("service_tier"), "senza «veloce» si usa il servizio normale: {args}");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn live_cfg() -> AgentConfig {
        crate::settings::Settings::default().agent()
    }

    /// Chiama davvero codex: `cargo test --release live_agent -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_agent_keeps_node_ids_stable_and_asks_about_real_nodes() {
        let dir = std::env::temp_dir().join(format!("rr-live-{}", std::process::id()));
        let mut s = Session::default();
        s.add_turn(Speaker::Io, "Come arrivano gli ordini?");
        s.add_turn(Speaker::Cliente, "Per email o telefono. La segretaria li mette nel gestionale, poi qualcuno avvisa il magazzino.");
        s.add_turn(Speaker::Io, "Chi li avvisa?");
        s.add_turn(Speaker::Cliente, "Dipende, a volte Paolo, a volte Marta. Dipende da chi c'è.");
        let first = run(&s, &dir, &live_cfg()).expect("primo giro");
        println!("PRIMO GIRO\n{}\nDOMANDE: {:?}", first.mermaid, first.questions);
        assert!(rune_mermaid::supported(&first.mermaid), "diagramma non disegnabile:\n{}", first.mermaid);
        for q in &first.questions {
            if let Some(n) = &q.node_id {
                assert!(crate::state::has_node(&first.mermaid, n), "domanda su nodo inesistente {n}");
            }
        }

        s.mermaid = first.mermaid.clone();
        s.summary = first.summary.clone();
        for q in &first.questions {
            let id = s.add_question(q.node_id.clone(), q.text.clone());
            s.answer(id, "Di solito Paolo al mattino, Marta al pomeriggio");
        }
        s.add_turn(Speaker::Cliente, "Poi il magazzino prepara l'ordine e lo spedisce con il corriere.");
        let second = run(&s, &dir, &live_cfg()).expect("secondo giro");
        println!("SECONDO GIRO\n{}\nDOMANDE: {:?}", second.mermaid, second.questions);
        assert!(rune_mermaid::supported(&second.mermaid));
        let ids = |m: &str| -> std::collections::BTreeSet<String> {
            m.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() >= 2 && w.starts_with('n') && w[1..].chars().all(|c| c.is_ascii_digit())).map(String::from).collect()
        };
        let (a, b) = (ids(&first.mermaid), ids(&second.mermaid));
        assert!(a.is_subset(&b), "id spariti o rinumerati: {a:?} -> {b:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Chiama davvero codex: la modifica richiesta su un nodo finisce nel diagramma.
    #[test]
    #[ignore]
    fn live_agent_applies_a_requested_edit_and_keeps_the_other_ids() {
        let dir = std::env::temp_dir().join(format!("rr-live-edit-{}", std::process::id()));
        let mut s = Session::default();
        s.add_turn(Speaker::Cliente, "Gli ordini arrivano per email, la segretaria li inserisce e poi li stampa per il magazzino.");
        s.mermaid = "flowchart TD\n  n1[Ordine ricevuto] --> n2[Segretaria inserisce nel gestionale]\n  n2 --> n3[Stampa foglio ordine]\n  n3 --> n4[Magazzino prepara]".into();
        s.add_instruction("n3", "Dopo questo passo aggiungi un controllo di qualità sul foglio stampato");
        let r = run(&s, &dir, &live_cfg()).expect("giro");
        println!("{}\nDOMANDE: {:?}", r.mermaid, r.questions);
        assert!(rune_mermaid::supported(&r.mermaid));
        assert!(r.mermaid.to_lowercase().contains("qualit"), "la modifica non è nel diagramma:\n{}", r.mermaid);
        for id in ["n1", "n2", "n3", "n4"] {
            assert!(crate::state::has_node(&r.mermaid, id), "{id} è sparito:\n{}", r.mermaid);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
