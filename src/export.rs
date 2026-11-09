//! L'esportazione del colloquio in PDF.
//!
//! GPT (lo stesso modello dell'agente) scrive la struttura e l'analisi del verbale in HTML, con
//! segnaposto dove deve comparire il materiale originale. Il programma inserisce quel materiale
//! parola per parola (trascrizione, domande e risposte, note, allegati, diagramma): il modello
//! non lo riscrive e non può accorciarlo. Chromium stampa poi l'HTML in PDF.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

use base64::Engine;
use serde_json::Value;

use crate::agent;
use crate::brand::Brand;
use crate::model::{Project, QStatus, Session};
use crate::settings::AgentConfig;
use crate::state::node_label;

const CODEX_TIMEOUT: Duration = Duration::from_secs(420);
/// I tipi di materiale che il programma inserisce al posto dei segnaposto.
const KINDS: [&str; 5] = ["DIAGRAMMA", "DOMANDE", "NOTE", "ALLEGATI", "TRASCRIZIONE"];

/// Le istruzioni di sistema per il modello: ruolo, fedeltà ai materiali, struttura, segnaposto, stile.
const SYSTEM_PROMPT: &str = r#"# Ruolo
Sei un analista di business e redattore tecnico di TwoBee, una società di consulenza digitale. Scrivi il verbale professionale di un incontro di raccolta requisiti con un cliente (CRM, automazioni aziendali, siti web). Lo leggeranno il consulente e il cliente, e da lì partirà la progettazione.

# Cosa devi produrre
Un unico documento HTML5 completo, in italiano, pronto per la stampa su A4. Rispondi SOLO con l'HTML, dal `<!DOCTYPE html>` a `</html>`: niente testo prima o dopo, niente blocchi ```. Non eseguire comandi e non leggere file: lavora solo sui materiali qui sotto.

# Regole di fedeltà (le più importanti)
- Usa soltanto ciò che sta nei materiali. Non inventare numeri, nomi, strumenti, volumi, scadenze, prezzi o decisioni.
- Se una cosa è una tua deduzione e non è stata detta in modo esplicito, scrivila come «da confermare».
- Distingui sempre: ciò che il cliente ha detto, ciò che ha risposto alle domande del consulente, ciò che resta aperto.
- Le domande senza risposta sono punti aperti: elencale così, non rispondere tu al posto del cliente.
- Collega ogni risposta al passo del processo a cui si riferisce, nell'analisi.
- Se la trascrizione è confusa o contraddittoria, dillo con una nota breve invece di scegliere una versione.
- Un nome o una parola che sembra un errore di trascrizione (es. nomi di strumenti) va scritto com'è, con «(sic)» solo se serve a evitare equivoci.

# Struttura del documento
1. Frontespizio: titolo «Verbale di raccolta requisiti», cliente, data, elenco delle aree trattate con l'interlocutore di ciascuna.
2. Sintesi esecutiva: al massimo 8 righe, con le 3-5 cose che contano di più.
3. Per ogni area, nell'ordine dato, una sezione con: titolo (area e interlocutore), Contesto, Processo attuale (descrivi in prosa breve e in passi numerati il processo del diagramma, poi il segnaposto del diagramma), Criticità e rischi, Requisiti emersi (elenco numerato R1, R2, … con priorità proposta alta/media/bassa e la fonte: «detto dal cliente», «risposta a domanda» o «da confermare»), Opportunità di automazione o di CRM (solo se ricavabili dai materiali), Punti aperti e domande (segnaposto delle domande), Prossimi passi proposti.
4. Appendici, una per area: note del consulente e allegati (se ci sono), poi la trascrizione completa (segnaposto).
Se un'area ha pochissimo materiale, la sua sezione è breve: non riempire.

# Segnaposto (materiale originale)
Dove vuoi che compaia il materiale originale scrivi ESATTAMENTE il segnaposto, da solo su una riga, senza altro attorno. Il programma lo sostituisce con il contenuto fedele: non riscrivere, non riassumere e non ripetere quel contenuto. Per l'area con numero N i segnaposto sono:
{{DIAGRAMMA:N}} (diagramma del processo), {{DOMANDE:N}} (tabella delle domande e risposte), {{NOTE:N}} (note del consulente), {{ALLEGATI:N}} (allegati), {{TRASCRIZIONE:N}} (trascrizione completa).
Usa solo i segnaposto che i materiali indicano come disponibili.

# Impaginazione
- Tutto il CSS in un solo `<style>` nel `<head>`. Pagina A4 con `@page` e margini di 18 mm.
- Corpo del testo 10,5 pt, interlinea 1,45; tabelle con bordi sottili.
- Niente JavaScript, niente immagini, font o link esterni, niente emoji. Non incorporare immagini: compaiono tramite i segnaposto.
- Evita pagine quasi vuote: `page-break-before` solo per le appendici; titoli con `page-break-after: avoid`.
"#;

pub type Delivery = Result<PathBuf, String>;

/// Gli strumenti esterni; `from_env` li legge dall'ambiente, i test ne passano di finti.
pub struct Tools {
    /// L'agente che scrive il verbale: lo stesso modello scelto nelle impostazioni.
    pub agent: AgentConfig,
    pub chromium: String,
    pub ffmpeg: String,
    /// Il programma che disegna un diagramma: di norma questo stesso.
    pub diagram_bin: Option<PathBuf>,
    pub brand: Brand,
}

impl Tools {
    pub fn from_env(agent: AgentConfig) -> Tools {
        Tools {
            agent,
            chromium: std::env::var("RR_CHROMIUM").unwrap_or_else(|_| "chromium".into()),
            ffmpeg: crate::audio::default_bin(),
            diagram_bin: std::env::var_os("RR_DIAGRAM_BIN").map(PathBuf::from).or_else(|| std::env::current_exe().ok()),
            brand: Brand::load(),
        }
    }
}

pub fn spawn(project: Project, dir: PathBuf, agent: AgentConfig, tx: Sender<Delivery>, wake: rune::Waker) {
    std::thread::spawn(move || {
        let _ = tx.send(run(&project, &dir, &Tools::from_env(agent), now_unix()));
        wake();
    });
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ---- date e nomi ----

/// Giorno civile (UTC) da secondi Unix.
fn civil(unix: u64) -> (i64, u32, u32) {
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn date_it(unix: u64) -> String {
    const MONTHS: [&str; 12] = ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];
    let (y, m, d) = civil(unix);
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

fn date_iso(unix: u64) -> String {
    let (y, m, d) = civil(unix);
    format!("{y:04}-{m:02}-{d:02}")
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Quando si è tenuto l'incontro: il primo audio registrato, altrimenti `fallback`.
fn meeting_unix(project: &Project, fallback: u64) -> u64 {
    project.areas.iter().flat_map(|a| a.segments.iter().map(|s| s.started)).filter(|&t| t > 0).min().unwrap_or(fallback)
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---- il prompt ----

fn status_it(q: &crate::model::Question) -> &'static str {
    match (q.status, q.answer.as_deref()) {
        (QStatus::Answered, Some(_)) => "risposta",
        _ => "senza risposta",
    }
}

/// Il manuale del brand e come applicarlo: il modello lo deve rispettare senza eccezioni.
fn brand_section(brand: &Brand) -> String {
    format!(
        "\n# Brand TwoBee (obbligatorio)\nIl documento è di TwoBee e deve rispettare il manuale del brand qui sotto, senza eccezioni: non cambiare i colori o i font e non aggiungerne altri.\n\n<manuale-brand>\n{}\n</manuale-brand>\n\nCome applicarlo:\n\
- Il programma incorpora i font nel documento e definisce queste variabili CSS: --tb-yellow, --tb-yellow-light, --tb-yellow-dark, --tb-black, --tb-dark, --tb-offwhite, --tb-white, --tb-font-title, --tb-font-body. Usale (per esempio `color: var(--tb-black)`) invece di scrivere i codici dei colori o i nomi dei font; non importare font e non nominarne altri.\n\
- I titoli (h1-h4) sono in {} peso 800 (`font-family: var(--tb-font-title)`); il testo è in {} (`font-family: var(--tb-font-body)`).\n\
- Le pagine hanno sfondo bianco (o off-white) e testo nero brand. Il giallo è l'accento: filetti, evidenziazioni, sfondi di etichette (anche --tb-yellow-light), segni decorativi. Mai testo giallo su sfondo chiaro, perché non si legge. Su sfondo nero il testo è off-white e il giallo può essere usato anche per il testo.\n\
- Frontespizio: un `<section class=\"tb-cover\">` (il programma lo rende a pagina intera, nero brand) con dentro un `<h1>` con il titolo, il cliente, la data e l'elenco delle aree. Il resto del documento sta su sfondo chiaro.\n\
- Classi già pronte: `<span class=\"tb-hex\"></span>` è un esagono giallo da usare come segno decorativo (i titoli h2 ne hanno già uno). Le tabelle e i segnaposto hanno già lo stile del brand.\n\
- Niente altri colori d'accento (niente blu, rosso o verde) e niente sfumature.\n",
        brand.manual, brand.title_font, brand.body_font
    )
}

pub fn build_prompt(project: &Project, date: &str, diagrams: &HashMap<u32, PathBuf>, brand: &Brand) -> String {
    let mut p = String::from(SYSTEM_PROMPT);
    p.push_str(&brand_section(brand));
    p.push_str("\n# Materiali\n");
    p.push_str(&format!("Cliente: {}\nData dell'incontro: {date}\n", if project.client.trim().is_empty() { "non indicato" } else { project.client.trim() }));
    p.push_str(&format!("Aree trattate: {}\n", project.areas.len()));
    for a in &project.areas {
        let id = a.id;
        p.push_str(&format!("\n## AREA {id}: {}\n", if a.name.trim().is_empty() { "senza nome" } else { a.name.trim() }));
        p.push_str(&format!("Interlocutore: {}\n", if a.interlocutor.trim().is_empty() { "non indicato" } else { a.interlocutor.trim() }));
        let mut avail = Vec::new();
        if diagrams.contains_key(&id) {
            avail.push(format!("{{{{DIAGRAMMA:{id}}}}}"));
        }
        if !a.questions.is_empty() {
            avail.push(format!("{{{{DOMANDE:{id}}}}} ({} domande)", a.questions.len()));
        }
        if !a.notes.trim().is_empty() {
            avail.push(format!("{{{{NOTE:{id}}}}}"));
        }
        if !a.attachments.is_empty() {
            avail.push(format!("{{{{ALLEGATI:{id}}}}} ({})", a.attachments.len()));
        }
        if !a.turns.is_empty() {
            avail.push(format!("{{{{TRASCRIZIONE:{id}}}}} ({} battute)", a.turns.len()));
        }
        p.push_str(&format!("Segnaposto disponibili per questa area: {}\n", if avail.is_empty() { "nessuno".to_string() } else { avail.join(", ") }));
        let mins = (a.audio_secs() / 60.0).round() as u32;
        if mins > 0 {
            p.push_str(&format!("Audio registrato: circa {mins} minuti\n"));
        }
        if !a.summary.trim().is_empty() {
            p.push_str(&format!("\n### Riassunto preparato dall'assistente durante il colloquio\n{}\n", a.summary.trim()));
        }
        if !a.open_points.is_empty() {
            p.push_str("\n### Punti segnalati come poco chiari dall'assistente\n");
            for o in &a.open_points {
                p.push_str(&format!("- {o}\n"));
            }
        }
        if !a.mermaid.trim().is_empty() {
            p.push_str(&format!("\n### Diagramma del processo (sorgente Mermaid; l'immagine la inserisce il programma)\n{}\n", a.mermaid.trim()));
        }
        if !a.questions.is_empty() {
            p.push_str("\n### Domande fatte al consulente durante il colloquio, con le sue risposte\n");
            for q in &a.questions {
                let node = q.node.as_deref().map_or("generale".to_string(), |n| node_label(&a.mermaid, n).unwrap_or_else(|| n.to_string()));
                p.push_str(&format!("- [passo: {node}] {} → {}\n", q.text, match q.answer.as_deref() { Some(x) => format!("risposta: {x}"), None => status_it(q).to_string() }));
            }
        }
        if !a.notes.trim().is_empty() {
            p.push_str(&format!("\n### Note del consulente\n{}\n", a.notes.trim()));
        }
        let described: Vec<_> = a.attachments.iter().filter_map(|x| x.summary.as_ref().map(|s| (&x.name, s))).collect();
        if !described.is_empty() {
            p.push_str("\n### Allegati (sintesi)\n");
            for (name, s) in described {
                p.push_str(&format!("- {name}: {s}\n"));
            }
        }
        p.push_str("\n### Trascrizione completa\n");
        if a.turns.is_empty() {
            p.push_str("(vuota)\n");
        }
        for t in &a.turns {
            p.push_str(&format!("[{}] {}\n", a.speaker_name(t.speaker), t.text));
        }
    }
    p
}

// ---- il diagramma ----

/// Il rettangolo `[x, y, w, h]` (in punti) dell'elemento con quella chiave, dall'albero salvato da `--tree`.
pub fn crop_rect(tree: &str, key: &str) -> Option<[f32; 4]> {
    let v: Value = serde_json::from_str(tree).ok()?;
    let el = v["elements"].as_array()?.iter().find(|e| e["key"] == key)?;
    let r = el["rect"].as_array()?;
    Some([r.first()?.as_f64()? as f32, r.get(1)?.as_f64()? as f32, r.get(2)?.as_f64()? as f32, r.get(3)?.as_f64()? as f32])
}

/// Disegna il diagramma con lo stesso codice dell'app (tema chiaro, senza finestra) e lo ritaglia.
fn render_diagram(tools: &Tools, mermaid: &str, work: &Path, tag: u32) -> Option<PathBuf> {
    if mermaid.trim().is_empty() || !rune_mermaid::supported(mermaid) {
        return None;
    }
    let exe = tools.diagram_bin.as_ref()?;
    let src = work.join(format!("diagramma-{tag}.mmd"));
    let (full, tree, out) = (work.join(format!("diagramma-{tag}-pieno.png")), work.join(format!("diagramma-{tag}.json")), work.join(format!("diagramma-{tag}.png")));
    std::fs::write(&src, mermaid).ok()?;
    // Una finestra abbastanza alta per il diagramma, entro il limite delle texture.
    let height = (mermaid.lines().count() as f32 * 70.0 + 500.0).clamp(1200.0, 3900.0) as u32;
    let drawn = Command::new(exe)
        .args(["--scene", "export-diagram", "--snap"])
        .arg(&full)
        .arg("--tree")
        .arg(&tree)
        .args(["--size", &format!("1900x{height}"), "--scale", "2", "--theme", "light"])
        .env("RR_EXPORT_MERMAID", &src)
        .env_remove("RR_RESUME")
        .env_remove("RR_SESSION")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !drawn.success() {
        return None;
    }
    let [x, y, w, h] = crop_rect(&std::fs::read_to_string(&tree).ok()?, "diagram")?;
    let margin = 16.0; // il margine della scheda attorno al diagramma
    let px = |v: f32| (v * 2.0).round().max(0.0) as u32;
    let filter = format!("crop={}:{}:{}:{}", px(w + 2.0 * margin), px(h + 2.0 * margin), px(x - margin), px(y - margin));
    let cropped = Command::new(&tools.ffmpeg).args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(&full).arg("-vf").arg(filter).arg(&out).stdin(Stdio::null()).status().ok()?;
    (cropped.success() && out.exists()).then_some(out)
}

// ---- l'HTML finale ----

/// Lo stile dei blocchi che inserisce il programma, con i colori del brand.
const COMPONENT_CSS: &str = r#"
@page { size: A4; margin: 18mm 16mm; }
.rr-turn { display: flex; gap: 10px; margin: 0 0 5px; break-inside: avoid; font-size: 9.5pt; line-height: 1.4; }
.rr-who { flex: 0 0 105px; font-family: var(--tb-font-title); font-weight: 800; font-size: 8.5pt; letter-spacing: .04em; text-transform: uppercase; color: var(--tb-black); padding-top: 1px; }
.rr-who:not(.rr-io)::before { content: ""; display: inline-block; width: .6em; height: .68em; margin-right: .4em; background: var(--tb-yellow); clip-path: polygon(50% 0, 100% 25%, 100% 75%, 50% 100%, 0 75%, 0 25%); }
.rr-text { flex: 1; }
.rr-diagram { margin: 10px 0; text-align: center; break-inside: avoid; }
.rr-diagram img { max-width: 100%; max-height: 225mm; border: 1px solid #e4e1db; border-radius: 6px; }
.rr-diagram figcaption { font-size: 8.5pt; color: #6b6a68; margin-top: 4px; }
table.rr-qa { width: 100%; font-size: 9.5pt; margin: 8px 0; }
table.rr-qa th, table.rr-qa td { padding: 5px 7px; vertical-align: top; text-align: left; }
table.rr-qa tr { break-inside: avoid; }
.rr-open { background: var(--tb-yellow-light); color: var(--tb-black); font-weight: 600; padding: 1px 6px; border-radius: 3px; white-space: nowrap; }
.rr-notes { white-space: pre-wrap; background: var(--tb-offwhite); padding: 8px 10px; border-left: 4px solid var(--tb-yellow); font-size: 9.5pt; margin: 8px 0; }
.rr-attach { font-size: 9.5pt; }
.rr-empty { color: #6b6a68; font-style: italic; }
.rr-appendix { break-before: page; }
"#;

fn heading(kind: &str) -> &'static str {
    match kind {
        "DIAGRAMMA" => "Diagramma del processo",
        "DOMANDE" => "Domande e risposte",
        "NOTE" => "Note del consulente",
        "ALLEGATI" => "Allegati",
        _ => "Trascrizione completa",
    }
}

fn has_content(kind: &str, a: &Session, diagram: bool) -> bool {
    match kind {
        "DIAGRAMMA" => diagram,
        "DOMANDE" => !a.questions.is_empty(),
        "NOTE" => !a.notes.trim().is_empty(),
        "ALLEGATI" => !a.attachments.is_empty(),
        _ => !a.turns.is_empty(),
    }
}

/// L'HTML del materiale originale, fedele e con il testo reso innocuo.
fn component(kind: &str, a: &Session, diagram: Option<&str>) -> String {
    match kind {
        "DIAGRAMMA" => match diagram {
            Some(uri) => format!("<figure class=\"rr-diagram\"><img src=\"{uri}\" alt=\"Diagramma del processo\"><figcaption>Diagramma del processo — {}</figcaption></figure>", escape(&a.name)),
            None => "<p class=\"rr-empty\">Nessun diagramma disponibile.</p>".into(),
        },
        "DOMANDE" => {
            if a.questions.is_empty() {
                return "<p class=\"rr-empty\">Nessuna domanda.</p>".into();
            }
            let mut rows = String::new();
            for q in &a.questions {
                let step = q.node.as_deref().map_or("Generale".to_string(), |n| node_label(&a.mermaid, n).unwrap_or_else(|| n.to_string()));
                let answer = match q.answer.as_deref() {
                    Some(x) => escape(x),
                    None => "<span class=\"rr-open\">Senza risposta</span>".into(),
                };
                rows.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{answer}</td></tr>", escape(&step), escape(&q.text)));
            }
            format!("<table class=\"rr-qa\"><thead><tr><th>Passo del processo</th><th>Domanda</th><th>Risposta</th></tr></thead><tbody>{rows}</tbody></table>")
        }
        "NOTE" => format!("<div class=\"rr-notes\">{}</div>", escape(a.notes.trim())),
        "ALLEGATI" => {
            let items: String = a.attachments.iter().map(|x| format!("<li><strong>{}</strong>{}</li>", escape(&x.name), x.summary.as_ref().map_or(String::new(), |s| format!(": {}", escape(s))))).collect();
            format!("<ul class=\"rr-attach\">{items}</ul>")
        }
        _ => {
            let turns: String = a
                .turns
                .iter()
                .map(|t| format!("<div class=\"rr-turn\"><span class=\"rr-who{}\">{}</span><span class=\"rr-text\">{}</span></div>", if t.speaker == crate::model::Speaker::Io { " rr-io" } else { "" }, escape(&a.speaker_name(t.speaker)), escape(&t.text)))
                .collect();
            format!("<section class=\"rr-transcript\">{turns}</section>")
        }
    }
}

/// Toglie il contorno che il modello può aggiungere (testo attorno, ```), e ciò che non deve
/// stare in un verbale stampato: script, frame, oggetti e link esterni.
fn clean(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    let lower = s.to_ascii_lowercase();
    if let Some(start) = lower.find("<!doctype").or_else(|| lower.find("<html")) {
        s = s[start..].to_string();
    }
    if let Some(end) = s.to_ascii_lowercase().rfind("</html>") {
        s.truncate(end + "</html>".len());
    }
    for tag in ["script", "iframe", "object", "embed", "style-remote"] {
        loop {
            let low = s.to_ascii_lowercase();
            let Some(open) = low.find(&format!("<{tag}")) else { break };
            let close_tag = format!("</{tag}>");
            let end = low[open..].find(&close_tag).map_or_else(|| low[open..].find('>').map_or(s.len(), |i| open + i + 1), |i| open + i + close_tag.len());
            s.replace_range(open..end, "");
        }
    }
    // Un `<link>` non serve (niente fogli di stile o font esterni).
    loop {
        let low = s.to_ascii_lowercase();
        let Some(open) = low.find("<link") else { break };
        let end = low[open..].find('>').map_or(s.len(), |i| open + i + 1);
        s.replace_range(open..end, "");
    }
    if !s.to_ascii_lowercase().contains("<html") {
        s = format!("<!DOCTYPE html><html lang=\"it\"><head></head><body>{s}</body></html>");
    }
    s
}

/// Toglie il `<p>` che il modello mette attorno a un segnaposto.
fn unwrap_paragraph(html: &str, token: &str) -> String {
    let mut out = html.to_string();
    for (open, close) in [("<p>", "</p>"), ("<p class=\"placeholder\">", "</p>")] {
        for pad in ["", "\n", " "] {
            out = out.replace(&format!("{open}{pad}{token}{pad}{close}"), token);
        }
    }
    out
}

/// Sostituisce i segnaposto con il materiale originale, aggiunge in appendice quello che il
/// modello ha dimenticato di richiamare e prepara la pagina per la stampa.
pub fn assemble(raw: &str, project: &Project, diagrams: &HashMap<u32, String>, brand: &Brand) -> String {
    let mut html = clean(raw);
    let mut appended = String::new();
    for a in &project.areas {
        let diagram = diagrams.get(&a.id).map(String::as_str);
        for kind in KINDS {
            let token = format!("{{{{{kind}:{}}}}}", a.id);
            html = unwrap_paragraph(&html, &token);
            if html.contains(&token) {
                html = html.replace(&token, &component(kind, a, diagram));
            } else if has_content(kind, a, diagram.is_some()) {
                appended.push_str(&format!("<section class=\"rr-appendix\"><h2>Appendice — {}: {}</h2>{}</section>", escape(&a.name), heading(kind), component(kind, a, diagram)));
            }
        }
    }
    // Un segnaposto che non corrisponde a niente (area inesistente) sparisce.
    while let Some(start) = html.find("{{") {
        let Some(len) = html[start..].find("}}") else { break };
        html.replace_range(start..start + len + 2, "");
    }
    let low = html.to_ascii_lowercase();
    if !appended.is_empty() {
        let at = low.rfind("</body>").unwrap_or(html.len());
        html.insert_str(at, &appended);
    }
    // Dopo lo stile del modello, così il brand prevale; la prima pagina senza margini solo se c'è il frontespizio.
    let head = format!(
        "<meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:\"><style>{COMPONENT_CSS}</style><style>{}</style>",
        brand.css(html.contains("tb-cover"))
    );
    let low = html.to_ascii_lowercase();
    match low.find("</head>") {
        Some(i) => html.insert_str(i, &head),
        None => match low.find("<body") {
            Some(i) => html.insert_str(i, &format!("<head>{head}</head>")),
            None => html.insert_str(0, &format!("<head>{head}</head>")),
        },
    }
    html
}

// ---- il PDF ----

fn html_to_pdf(tools: &Tools, html: &Path, pdf: &Path, work: &Path) -> Result<(), String> {
    let profile = work.join("chromium");
    let _ = std::fs::remove_file(pdf);
    let url = format!("file://{}", html.display());
    let status = Command::new(&tools.chromium)
        .args(["--headless=new", "--disable-gpu", "--no-pdf-header-footer", "--disable-extensions", "--no-first-run"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--print-to-pdf={}", pdf.display()))
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("non riesco a lanciare chromium: {e}"))?;
    if status.success() && std::fs::metadata(pdf).is_ok_and(|m| m.len() > 0) {
        Ok(())
    } else {
        Err("chromium non ha prodotto il PDF".into())
    }
}

/// Dove va il PDF: `<sessione>/esportazioni/<cliente>-<data>.pdf`, senza sovrascrivere i precedenti.
fn pdf_path(dir: &Path, client: &str, when: u64) -> PathBuf {
    let folder = dir.join("esportazioni");
    let base = format!("{}-{}", if slug(client).is_empty() { "verbale".to_string() } else { slug(client) }, date_iso(when));
    let mut path = folder.join(format!("{base}.pdf"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{base}-{n}.pdf"));
        n += 1;
    }
    path
}

/// L'esportazione, bloccante: da eseguire in un thread. Torna il percorso del PDF.
pub fn run(project: &Project, dir: &Path, tools: &Tools, now: u64) -> Result<PathBuf, String> {
    if project.areas.iter().all(|a| a.turns.is_empty() && a.mermaid.trim().is_empty() && a.questions.is_empty()) {
        return Err("non c'è ancora niente da esportare".into());
    }
    let work = dir.join(".esporta");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("esportazioni")).map_err(|e| e.to_string())?;

    // 1. i diagrammi come immagini
    let mut pngs: HashMap<u32, PathBuf> = HashMap::new();
    for a in &project.areas {
        if let Some(png) = render_diagram(tools, &a.mermaid, &work, a.id) {
            pngs.insert(a.id, png);
        }
    }
    let uris: HashMap<u32, String> = pngs
        .iter()
        .filter_map(|(id, p)| Some((*id, format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(std::fs::read(p).ok()?)))))
        .collect();

    // 2. GPT scrive il verbale
    let when = meeting_unix(project, now);
    let prompt = build_prompt(project, &date_it(when), &pngs, &tools.brand);
    // Il verbale chiede più ragionamento di un giro sulla lavagna: almeno «medio».
    let effort = if tools.agent.effort == "high" { "high" } else { "medium" };
    let raw = agent::exec_text(&tools.agent, &prompt, &work.join("codex"), None, &[], effort, CODEX_TIMEOUT)?;
    if raw.trim().is_empty() {
        return Err("l'agente non ha scritto il documento".into());
    }

    // 3. il materiale originale al posto dei segnaposto, e il PDF
    let html = assemble(&raw, project, &uris, &tools.brand);
    let html_path = work.join("verbale.html");
    std::fs::write(&html_path, &html).map_err(|e| e.to_string())?;
    let pdf = pdf_path(dir, &project.client, when);
    html_to_pdf(tools, &html_path, &pdf, &work)?;
    // Una copia dell'HTML accanto al PDF, utile per ritoccarlo.
    let _ = std::fs::copy(&html_path, pdf.with_extension("html"));
    Ok(pdf)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::model::{Attachment, AttachmentKind, Speaker};

    fn project() -> Project {
        let mut a = Session::area(0, "CRM");
        a.interlocutor = "Marco, vendite".into();
        a.add_turn(Speaker::Io, "Come arrivano gli ordini?");
        a.add_turn(Speaker::Cliente, "Per email <b>e</b> telefono & WhatsApp.");
        a.mermaid = "flowchart TD\n  n1[Ordine] --> n2[Inserimento]".into();
        a.summary = "Gli ordini arrivano su più canali.".into();
        a.notes = "Chiedere del gestionale.".into();
        let q1 = a.add_question(Some("n2".into()), "Chi inserisce l'ordine?".into());
        a.add_question(None, "Quanti ordini al giorno?".into());
        a.answer(q1, "La segretaria");
        a.attachments.push(Attachment { name: "listino.pdf".into(), path: "listino.pdf".into(), kind: AttachmentKind::Document, summary: Some("prezzi 2026".into()) });
        let mut b = Session::area(1, "Automazione interna");
        b.add_turn(Speaker::Cliente, "Copio le fatture a mano.");
        Project { client: "Rossi Meccanica".into(), active: 0, next_area_id: 2, areas: vec![a, b] }
    }

    #[test]
    fn dates_are_correct_and_in_italian() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(1_000_000_000), (2001, 9, 9));
        assert_eq!(date_it(1_000_000_000), "9 settembre 2001");
        assert_eq!(date_iso(1_762_690_000), "2025-11-09");
        assert_eq!(civil(951_782_400), (2000, 2, 29), "bisestile");
    }

    #[test]
    fn file_names_are_plain_ascii() {
        assert_eq!(slug("Rossi Meccanica S.r.l."), "rossi-meccanica-s-r-l");
        assert_eq!(slug("  Società Più "), "societ-pi");
        assert_eq!(slug("???"), "");
    }

    #[test]
    fn the_prompt_has_the_rules_the_placeholders_and_the_material_verbatim() {
        let mut pngs = HashMap::new();
        pngs.insert(0, PathBuf::from("d.png"));
        let p = build_prompt(&project(), "9 novembre 2026", &pngs, &Brand::default());
        assert!(p.contains("Regole di fedeltà") && p.contains("Non inventare"));
        assert!(p.contains("Cliente: Rossi Meccanica") && p.contains("9 novembre 2026"));
        assert!(p.contains("## AREA 0: CRM") && p.contains("## AREA 1: Automazione interna"));
        assert!(p.contains("{{DIAGRAMMA:0}}") && p.contains("{{TRASCRIZIONE:0}} (2 battute)") && p.contains("{{DOMANDE:0}} (2 domande)"));
        assert!(!p.contains("{{DIAGRAMMA:1}}"), "l'area 1 non ha diagramma disponibile");
        assert!(p.contains("[Io] Come arrivano gli ordini?") && p.contains("[Marco, vendite] Per email <b>e</b> telefono & WhatsApp."));
        assert!(p.contains("[passo: Inserimento] Chi inserisce l'ordine? → risposta: La segretaria"));
        assert!(p.contains("Quanti ordini al giorno? → senza risposta"));
        assert!(p.contains("listino.pdf: prezzi 2026"));
    }

    #[test]
    fn placeholders_become_the_original_text_escaped_and_unmodified() {
        let raw = "```html\nIntro del modello\n<!DOCTYPE html><html><head><title>V</title></head><body><h1>Verbale</h1>\n<p>{{TRASCRIZIONE:0}}</p>\n{{DOMANDE:0}}\n{{NOTE:0}}\n{{ALLEGATI:0}}\n{{TRASCRIZIONE:1}}\n</body></html>\n```\nFine";
        let html = assemble(raw, &project(), &HashMap::new(), &Brand::default());
        assert!(html.starts_with("<!DOCTYPE html>"), "il testo prima dell'html sparisce");
        assert!(html.ends_with("</html>") && !html.contains("```") && !html.contains("Fine"));
        assert!(html.contains("Per email &lt;b&gt;e&lt;/b&gt; telefono &amp; WhatsApp."), "il testo del cliente è fedele e reso innocuo");
        assert!(!html.contains("<b>e</b>"));
        assert!(html.contains("Marco, vendite</span>"), "i nomi di chi parla");
        assert!(html.contains("<td>Inserimento</td>") && html.contains("<td>La segretaria</td>"), "il passo del processo e la risposta");
        assert!(html.contains("Senza risposta"));
        assert!(html.contains("Chiedere del gestionale."));
        assert!(!html.contains("{{") && !html.contains("<p><section"), "niente segnaposto e niente <p> attorno ai blocchi");
    }

    #[test]
    fn what_the_model_forgets_to_call_is_added_as_an_appendix() {
        let html = assemble("<html><head></head><body><h1>Solo una sintesi</h1></body></html>", &project(), &HashMap::new(), &Brand::default());
        assert!(html.contains("Appendice — CRM: Trascrizione completa") && html.contains("Appendice — Automazione interna: Trascrizione completa"));
        assert!(html.contains("Appendice — CRM: Domande e risposte"));
        assert!(html.contains("Copio le fatture a mano."), "nessuna parola della trascrizione si perde");
        assert!(!html.contains("Diagramma del processo</h2>"), "senza immagine non c'è appendice del diagramma");
    }

    #[test]
    fn the_diagram_goes_in_as_an_inline_image() {
        let mut d = HashMap::new();
        d.insert(0, "data:image/png;base64,AAAA".to_string());
        let html = assemble("<html><head></head><body>{{DIAGRAMMA:0}}</body></html>", &project(), &d, &Brand::default());
        assert!(html.contains("<img src=\"data:image/png;base64,AAAA\""));
        assert!(html.contains("Diagramma del processo — CRM"));
    }

    #[test]
    fn scripts_remote_links_and_unknown_placeholders_never_reach_the_page() {
        let raw = "<html><head><link rel=\"stylesheet\" href=\"https://x.test/a.css\"><script src=\"https://x.test/a.js\"></script></head><body><SCRIPT>alert(1)</SCRIPT><iframe src=\"https://x.test\"></iframe>Testo {{TRASCRIZIONE:99}}<p>{{TRASCRIZIONE:0}}</p></body></html>";
        let html = assemble(raw, &project(), &HashMap::new(), &Brand::default());
        let low = html.to_lowercase();
        for banned in ["<script", "alert(1)", "<link", "<iframe", "x.test", "{{"] {
            assert!(!low.contains(banned), "{banned} è rimasto: {html}");
        }
        assert!(low.contains("content-security-policy") && low.contains("default-src 'none'"), "la pagina non può caricare niente");
        assert!(html.contains("Testo "), "il resto resta");
    }

    #[test]
    fn a_reply_that_is_only_a_fragment_still_becomes_a_complete_page() {
        let html = assemble("<h1>Titolo</h1>{{TRASCRIZIONE:0}}", &project(), &HashMap::new(), &Brand::default());
        assert!(html.contains("<html") && html.contains("<head>") && html.contains("<style>") && html.contains("Come arrivano gli ordini?"));
    }

    #[test]
    fn the_crop_comes_from_the_element_tree() {
        let tree = r#"{"elements":[{"key":"canvas","rect":[0,0,10,10]},{"key":"diagram","rect":[24.5,30,300,420.25]}]}"#;
        assert_eq!(crop_rect(tree, "diagram"), Some([24.5, 30.0, 300.0, 420.25]));
        assert_eq!(crop_rect(tree, "manca"), None);
        assert_eq!(crop_rect("non json", "diagram"), None);
    }

    #[test]
    fn two_exports_the_same_day_do_not_overwrite_each_other() {
        let dir = std::env::temp_dir().join(format!("rr-pdfname-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("esportazioni")).unwrap();
        let first = pdf_path(&dir, "Rossi Meccanica", 1_762_690_000);
        assert!(first.ends_with("rossi-meccanica-2025-11-09.pdf"));
        std::fs::write(&first, "x").unwrap();
        assert!(pdf_path(&dir, "Rossi Meccanica", 1_762_690_000).ends_with("rossi-meccanica-2025-11-09-2.pdf"));
        assert!(pdf_path(&dir, "", 1_762_690_000).ends_with("verbale-2025-11-09.pdf"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_prompt_hands_the_brand_manual_to_the_model_and_names_the_css_variables() {
        let p = build_prompt(&project(), "oggi", &HashMap::new(), &Brand::default());
        assert!(p.contains("Brand TwoBee (obbligatorio)") && p.contains("<manuale-brand>") && p.contains("GIALLO BRAND") && p.contains("#FFC501") && p.contains("League Spartan"));
        for var in ["--tb-yellow", "--tb-black", "--tb-offwhite", "--tb-font-title", "--tb-font-body", "tb-cover", "tb-hex"] {
            assert!(p.contains(var), "manca {var}");
        }
        assert!(p.contains("Mai testo giallo su sfondo chiaro"));
        assert!(!p.contains("#1f4e79"), "il vecchio azzurro non c'è più");
    }

    #[test]
    fn a_manual_changed_on_disk_reaches_the_model_unchanged() {
        let custom = Brand::parse("COLORI\n  Default ..... #00AA11\nNOTA UNICA DI PROVA");
        let p = build_prompt(&project(), "oggi", &HashMap::new(), &custom);
        assert!(p.contains("NOTA UNICA DI PROVA") && !p.contains("GIALLO BRAND"));
    }

    #[test]
    fn the_brand_css_comes_after_the_models_own_style_so_it_wins() {
        let html = assemble("<html><head><style>body{font-family:Arial;color:red}</style></head><body>x</body></html>", &project(), &HashMap::new(), &Brand::default());
        let model = html.find("font-family:Arial").unwrap();
        let brand = html.find("--tb-yellow: #FFC501").unwrap();
        assert!(model < brand, "stesso peso: vince lo stile successivo");
        assert!(html.contains("font-src data:"), "i font incorporati sono permessi, quelli esterni no");
        assert!(html.contains("font-family: var(--tb-font-body) !important"));
    }

    #[test]
    fn the_first_page_loses_its_margins_only_when_the_document_has_a_cover() {
        let with = assemble("<html><head></head><body><section class=\"tb-cover\"><h1>V</h1></section></body></html>", &project(), &HashMap::new(), &Brand::default());
        let without = assemble("<html><head></head><body><h1>V</h1></body></html>", &project(), &HashMap::new(), &Brand::default());
        assert!(with.contains("@page :first") && !without.contains("@page :first"));
    }

    fn fake_agent(bin: String) -> AgentConfig {
        AgentConfig { bin, model: "gpt-6.1-terra".into(), effort: "low".into(), fast: true }
    }

    fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn the_whole_pipeline_with_a_fake_model_and_a_fake_browser_makes_the_pdf() {
        let dir = std::env::temp_dir().join(format!("rr-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Il finto codex salva la richiesta e scrive nel file di `-o` un verbale con i segnaposto.
        let codex = script(&dir, "codex.sh", &format!(
            "prev=\"\"; for a; do [ \"$prev\" = \"-o\" ] && out=\"$a\"; prev=\"$a\"; done\ncat > \"{0}/prompt-ricevuto.txt\"\ncat > \"$out\" <<'HTML'\n<!DOCTYPE html><html><head><title>V</title></head><body><h1>Verbale</h1>{{{{TRASCRIZIONE:0}}}}{{{{DOMANDE:0}}}}</body></html>\nHTML",
            dir.display()
        ));
        // Il finto chromium scrive un PDF all'indirizzo di `--print-to-pdf=`.
        let chromium = script(&dir, "chromium.sh", "for a; do case \"$a\" in --print-to-pdf=*) out=\"${a#--print-to-pdf=}\";; esac; done\nprintf '%%PDF-1.4 finto' > \"$out\"");
        let tools = Tools { agent: fake_agent(codex), chromium, ffmpeg: "ffmpeg".into(), diagram_bin: None, brand: Brand::default() };
        let pdf = run(&project(), &dir, &tools, 1_762_690_000).expect("esportazione");
        assert!(pdf.starts_with(dir.join("esportazioni")) && pdf.extension().unwrap() == "pdf");
        assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
        let prompt = std::fs::read_to_string(dir.join("prompt-ricevuto.txt")).unwrap();
        assert!(prompt.contains("Regole di fedeltà") && prompt.contains("[Marco, vendite] Per email"), "il modello riceve istruzioni e materiale");
        let html = std::fs::read_to_string(pdf.with_extension("html")).unwrap();
        assert!(html.contains("Per email &lt;b&gt;e&lt;/b&gt; telefono &amp; WhatsApp.") && html.contains("La segretaria"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_browser_or_model_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("rr-export-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tools = Tools { agent: fake_agent("/non/esiste/codex".into()), chromium: "/non/esiste/chromium".into(), ffmpeg: "ffmpeg".into(), diagram_bin: None, brand: Brand::default() };
        assert!(run(&project(), &dir, &tools, 1).unwrap_err().contains("codex"));
        assert!(run(&Project::single("Vuota"), &dir, &tools, 1).unwrap_err().contains("niente da esportare"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Esporta una sessione vera con il modello e il browser veri:
    /// `RR_EXPORT_DIR=<sessione> RR_DIAGRAM_BIN=./target/release/raccolta-requisiti cargo test --release live_export -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_export_makes_a_real_pdf() {
        let dir = PathBuf::from(std::env::var("RR_EXPORT_DIR").expect("serve RR_EXPORT_DIR"));
        let project = crate::model::load(&dir).expect("sessione");
        let pdf = run(&project, &dir, &Tools::from_env(crate::settings::Settings::default().agent()), now_unix()).expect("esportazione");
        println!("PDF: {}", pdf.display());
    }
}
