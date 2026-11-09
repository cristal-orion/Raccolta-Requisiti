//! L'archivio dei colloqui per il menu principale: tutte le sessioni salvate, raggruppate per
//! cliente, dalla più recente. Legge soltanto: non tocca i file delle sessioni.

use std::path::{Path, PathBuf};

use crate::model::{self, Project};

/// Un colloquio salvato, con quello che serve a riconoscerlo nell'elenco.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub dir: PathBuf,
    pub client: String,
    /// Le aree, ognuna con il suo interlocutore se c'è: «CRM (Marco)».
    pub areas: Vec<String>,
    pub turns: usize,
    pub questions: usize,
    /// Domande ancora senza risposta.
    pub pending: usize,
    pub audio_secs: f32,
    /// L'inizio del primo riassunto dell'agente.
    pub summary: String,
    /// Ultimo salvataggio (secondi Unix).
    pub modified: u64,
}

/// I colloqui di un cliente, dal più recente.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub client: String,
    pub entries: Vec<Entry>,
}

/// Il nome con cui si raggruppa chi non ha indicato il cliente.
pub const NO_CLIENT: &str = "Senza cliente";
const SUMMARY_CHARS: usize = 170;

fn entry(dir: &Path, p: &Project, modified: u64) -> Entry {
    let areas = p
        .areas
        .iter()
        .map(|a| if a.interlocutor.trim().is_empty() { a.name.clone() } else { format!("{} ({})", a.name, a.interlocutor.trim()) })
        .collect();
    let summary = p.areas.iter().map(|a| a.summary.trim()).find(|s| !s.is_empty()).unwrap_or("");
    let mut short: String = summary.chars().take(SUMMARY_CHARS).collect();
    if summary.chars().count() > SUMMARY_CHARS {
        short = format!("{}…", short.trim_end());
    }
    Entry {
        dir: dir.to_path_buf(),
        client: p.client.trim().to_string(),
        areas,
        turns: p.areas.iter().map(|a| a.turns.len()).sum(),
        questions: p.areas.iter().map(|a| a.questions.len()).sum(),
        pending: p.areas.iter().map(|a| a.pending_count()).sum(),
        audio_secs: p.areas.iter().map(|a| a.audio_secs()).sum(),
        summary: short,
        modified,
    }
}

/// Le sessioni salvate sotto `root`; le cartelle senza `session.json` non contano.
pub fn scan(root: &Path) -> Vec<Entry> {
    let Ok(dirs) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<Entry> = dirs
        .flatten()
        .filter(|d| d.path().is_dir())
        .filter_map(|d| {
            let dir = d.path();
            let modified = std::fs::metadata(dir.join("session.json")).ok()?.modified().ok()?;
            let unix = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_secs());
            Some(entry(&dir, &model::load(&dir)?, unix))
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

fn matches(e: &Entry, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty() || e.client.to_lowercase().contains(&f) || e.areas.iter().any(|a| a.to_lowercase().contains(&f)) || e.summary.to_lowercase().contains(&f)
}

/// Raggruppa per cliente (senza badare a maiuscole e spazi), i clienti dal più recente; chi non
/// ha cliente va in fondo. `filter` cerca in cliente, aree, interlocutori e riassunto.
pub fn group(entries: &[Entry], filter: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut sorted: Vec<&Entry> = entries.iter().filter(|e| matches(e, filter)).collect();
    sorted.sort_by(|a, b| b.modified.cmp(&a.modified));
    for e in sorted {
        let key = e.client.to_lowercase();
        match groups.iter_mut().find(|g| g.client.to_lowercase() == key || (key.is_empty() && g.client == NO_CLIENT)) {
            Some(g) => {
                // Tra «bianchi srl» e «Bianchi Srl» vince la grafia con le maiuscole.
                if g.client == g.client.to_lowercase() && e.client != e.client.to_lowercase() {
                    g.client = e.client.clone();
                }
                g.entries.push(e.clone());
            }
            None => groups.push(Group { client: if e.client.is_empty() { NO_CLIENT.into() } else { e.client.clone() }, entries: vec![e.clone()] }),
        }
    }
    // Senza cliente in fondo, gli altri restano dal più recente.
    groups.sort_by_key(|g| g.client == NO_CLIENT);
    groups
}

/// L'ora locale di un istante Unix: (anno, mese, giorno, ore, minuti).
fn local(unix: u64) -> (i32, u32, u32, u32, u32) {
    let t = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32)
}

const MONTHS: [&str; 12] = ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];

/// «oggi, 14:38», «ieri, 9:05», «3 novembre, 10:12», «3 novembre 2025».
pub fn when(then: (i32, u32, u32, u32, u32), today: (i32, u32, u32), yesterday: (i32, u32, u32)) -> String {
    let (y, m, d, hh, mm) = then;
    let day = (y, m, d);
    if day == today {
        format!("oggi, {hh}:{mm:02}")
    } else if day == yesterday {
        format!("ieri, {hh}:{mm:02}")
    } else if y == today.0 {
        format!("{d} {}, {hh}:{mm:02}", MONTHS[(m.clamp(1, 12) - 1) as usize])
    } else {
        format!("{d} {} {y}", MONTHS[(m.clamp(1, 12) - 1) as usize])
    }
}

/// Quando è stato salvato l'ultima volta, in parole, nell'ora locale.
pub fn when_saved(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let day = |u: u64| {
        let (y, m, d, _, _) = local(u);
        (y, m, d)
    };
    when(local(unix), day(now), day(now.saturating_sub(86_400)))
}

/// I numeri del colloquio in una riga: «19 battute · 21 domande, 5 in sospeso · 4 min di audio».
pub fn stats(e: &Entry) -> String {
    let mut parts = Vec::new();
    parts.push(match e.turns {
        0 => "nessuna battuta".to_string(),
        1 => "1 battuta".to_string(),
        n => format!("{n} battute"),
    });
    if e.questions > 0 {
        let q = if e.questions == 1 { "1 domanda".to_string() } else { format!("{} domande", e.questions) };
        parts.push(if e.pending > 0 { format!("{q}, {} in sospeso", e.pending) } else { q });
    }
    let mins = (e.audio_secs / 60.0).round() as u32;
    if e.audio_secs > 0.0 {
        parts.push(if mins == 0 { "meno di 1 min di audio".to_string() } else { format!("{mins} min di audio") });
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Session, Speaker};

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rr-library-{name}-{}", std::process::id()))
    }

    fn project(client: &str, area: &str, who: &str, turns: usize) -> Project {
        let mut a = Session::area(0, area);
        a.interlocutor = who.into();
        for i in 0..turns {
            a.add_turn(Speaker::Cliente, &format!("battuta {i}"));
        }
        a.summary = "Gli ordini arrivano per email.".into();
        let q = a.add_question(None, "Chi?".into());
        a.add_question(None, "Quando?".into());
        a.answer(q, "Paolo");
        Project { client: client.into(), active: 0, next_area_id: 1, areas: vec![a] }
    }

    fn e(client: &str, area: &str, modified: u64) -> Entry {
        Entry { dir: PathBuf::from(format!("/s/{area}")), client: client.into(), areas: vec![area.into()], turns: 0, questions: 0, pending: 0, audio_secs: 0.0, summary: String::new(), modified }
    }

    #[test]
    fn scanning_finds_saved_sessions_newest_first_and_skips_other_folders() {
        let root = temp("scan");
        let _ = std::fs::remove_dir_all(&root);
        model::save(&root.join("rossana"), &project("Rossi", "Area 2", "Rossana", 15)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        model::save(&root.join("bianchi"), &project("Bianchi Srl", "CRM", "", 2)).unwrap();
        std::fs::create_dir_all(root.join("vuota")).unwrap();
        let list = scan(&root);
        assert_eq!(list.len(), 2, "la cartella senza session.json non conta");
        assert_eq!(list[0].client, "Bianchi Srl", "la più recente per prima");
        let r = &list[1];
        assert_eq!((r.areas[0].as_str(), r.turns, r.questions, r.pending), ("Area 2 (Rossana)", 15, 2, 1));
        assert_eq!(list[0].areas[0], "CRM", "senza interlocutore solo il nome");
        assert!(scan(&root.join("non-esiste")).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sessions_group_by_client_regardless_of_case_and_those_without_go_last() {
        let list = [e("Rossi", "CRM", 50), e("", "Prova", 90), e("Bianchi", "Sito", 80), e("rossi ", "Magazzino", 100)];
        let list: Vec<Entry> = list.into_iter().map(|mut x| {
            x.client = x.client.trim().to_string();
            x
        }).collect();
        let g = group(&list, "");
        assert_eq!(g.iter().map(|g| g.client.as_str()).collect::<Vec<_>>(), ["Rossi", "Bianchi", NO_CLIENT], "con le maiuscole, se una grafia le ha");
        assert_eq!(g[0].entries.iter().map(|x| x.areas[0].as_str()).collect::<Vec<_>>(), ["Magazzino", "CRM"]);
    }

    #[test]
    fn the_filter_looks_in_client_areas_and_people() {
        let mut a = e("Rossi", "CRM (Marco)", 10);
        a.summary = "ordini via WhatsApp".into();
        let list = [a, e("Bianchi", "Sito (Laura)", 20)];
        assert_eq!(group(&list, "laura")[0].client, "Bianchi");
        assert_eq!(group(&list, "ROSSI")[0].client, "Rossi");
        assert_eq!(group(&list, "whatsapp")[0].client, "Rossi");
        assert!(group(&list, "nessuno").is_empty());
    }

    #[test]
    fn dates_read_as_today_yesterday_or_a_day() {
        let today = (2026, 11, 9);
        let yesterday = (2026, 11, 8);
        assert_eq!(when((2026, 11, 9, 14, 38), today, yesterday), "oggi, 14:38");
        assert_eq!(when((2026, 11, 8, 9, 5), today, yesterday), "ieri, 9:05");
        assert_eq!(when((2026, 11, 3, 10, 12), today, yesterday), "3 novembre, 10:12");
        assert_eq!(when((2025, 3, 1, 10, 12), today, yesterday), "1 marzo 2025");
    }

    #[test]
    fn stats_say_only_what_there_is() {
        let mut x = e("R", "A", 0);
        assert_eq!(stats(&x), "nessuna battuta");
        x.turns = 19;
        x.questions = 21;
        x.pending = 5;
        x.audio_secs = 250.0;
        assert_eq!(stats(&x), "19 battute · 21 domande, 5 in sospeso · 4 min di audio");
        x.pending = 0;
        x.audio_secs = 20.0;
        assert_eq!(stats(&x), "19 battute · 21 domande · meno di 1 min di audio");
    }
}
