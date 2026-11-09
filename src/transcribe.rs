//! La trascrizione: manda a Gemini (il modello scelto nelle impostazioni, di norma
//! `gemini-3.5-transcribe`, con separazione delle voci) l'audio di una finestra e riporta le frasi
//! con chi le ha dette. Le chiamate di rete le fa `curl`: la chiave passa dalla sua configurazione
//! su stdin e non compare mai tra gli argomenti.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use base64::Engine;
use serde_json::{Value, json};

const URL: &str = "https://generativelanguage.googleapis.com/v1beta/interactions";
const MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models/";

pub struct Job {
    pub area: u32,
    pub window: String,
    /// I pezzi della finestra, in ordine, con il percorso completo.
    pub files: Vec<PathBuf>,
    pub work_dir: PathBuf,
    pub key: String,
    pub model: String,
    pub lang: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Utterance {
    /// Il numero della voce (`spk:0` → 0): la prima a parlare nella finestra è la 0.
    pub speaker: u32,
    pub text: String,
}

/// Area, finestra ed esito.
pub type Outcome = (u32, String, Result<Vec<Utterance>, String>);

pub fn spawn(job: Job, tx: Sender<Outcome>, wake: rune::Waker) {
    std::thread::spawn(move || {
        let _ = tx.send((job.area, job.window.clone(), run(&job)));
        wake();
    });
}

pub fn run(job: &Job) -> Result<Vec<Utterance>, String> {
    std::fs::create_dir_all(&job.work_dir).map_err(|e| e.to_string())?;
    let audio = join_chunks(&job.files, &job.work_dir, &job.window)?;
    let bytes = std::fs::read(&audio).map_err(|e| format!("non riesco a leggere l'audio: {e}"))?;
    let body = request_body(&bytes, &job.model, &job.lang);
    let request = job.work_dir.join("richiesta.json");
    std::fs::write(&request, serde_json::to_vec(&body).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let reply = post(&job.key, &request);
    let _ = std::fs::remove_file(&request);
    parse_utterances(&reply?)
}

/// Un solo file se il pezzo è uno; altrimenti li unisce senza ricodificare.
fn join_chunks(files: &[PathBuf], work_dir: &Path, window: &str) -> Result<PathBuf, String> {
    match files {
        [] => Err("nessun audio da trascrivere".into()),
        [one] => Ok(one.clone()),
        many => {
            let list: String = many.iter().map(|f| format!("file '{}'\n", f.display().to_string().replace('\'', "'\\''"))).collect();
            let list_path = work_dir.join(format!("{window}.txt"));
            let out = work_dir.join(format!("{window}.ogg"));
            std::fs::write(&list_path, list).map_err(|e| e.to_string())?;
            let status = Command::new(crate::audio::default_bin())
                .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
                .arg(&list_path)
                .args(["-c", "copy"])
                .arg(&out)
                .status()
                .map_err(|e| format!("non riesco a unire i pezzi: {e}"))?;
            let _ = std::fs::remove_file(&list_path);
            if status.success() { Ok(out) } else { Err("non sono riuscito a unire i pezzi di audio".into()) }
        }
    }
}

fn request_body(audio: &[u8], model: &str, lang: &str) -> Value {
    json!({
        "model": model,
        "input": [{ "type": "audio", "data": base64::engine::general_purpose::STANDARD.encode(audio), "mime_type": "audio/ogg" }],
        "generation_config": { "transcription_config": {
            "language_codes": [lang],
            "mode": { "type": "verbatim", "diarization_mode": "speaker" }
        }}
    })
}

fn curl_bin() -> String {
    std::env::var("RR_CURL").unwrap_or_else(|_| "curl".into())
}

fn post(key: &str, request: &Path) -> Result<String, String> {
    post_with(&curl_bin(), key, request)
}

fn post_with(bin: &str, key: &str, request: &Path) -> Result<String, String> {
    let mut args: Vec<String> = ["-H", "Content-Type: application/json", "-d"].map(String::from).to_vec();
    args.push(format!("@{}", request.display()));
    let (code, body) = call(bin, key, &args, URL)?;
    if code == "200" {
        Ok(body)
    } else {
        Err(format!("Gemini ha risposto {code}: {}", error_message(&body)))
    }
}

/// Una richiesta a Gemini con la chiave nell'intestazione: torna il codice HTTP e il corpo.
fn call(bin: &str, key: &str, args: &[String], url: &str) -> Result<(String, String), String> {
    if key.contains(['"', '\n', '\r', '\\']) {
        return Err("la chiave Gemini contiene caratteri non validi".into());
    }
    let mut child = Command::new(bin)
        .args(["-sS", "-m", "240", "-K", "-"])
        .args(args)
        .args(["-w", "\n%{http_code}", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("non riesco a lanciare curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "header = \"x-goog-api-key: {key}\"");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("rete: {}", why.trim().lines().last().unwrap_or("curl è fallito")));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let text = text.trim_end();
    let (body, code) = text.rsplit_once('\n').unwrap_or((text, ""));
    Ok((code.trim().to_string(), body.to_string()))
}

/// L'esito della prova della chiave.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    /// Chiave valida e modello trovato.
    Ok(String),
    /// Chiave valida, ma qualcosa da controllare.
    Warn(String),
}

/// Prova la chiave chiedendo a Gemini la scheda del modello: non trascrive niente e non costa.
pub fn check(key: &str, model: &str) -> Result<Check, String> {
    check_with(&curl_bin(), key, model)
}

fn check_with(bin: &str, key: &str, model: &str) -> Result<Check, String> {
    if let Some(why) = crate::settings::key_problem(key) {
        return Err(why.into());
    }
    let model = model.trim().trim_start_matches("models/");
    if model.is_empty() || !model.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) {
        return Err("il nome del modello non è valido".into());
    }
    let (code, body) = call(bin, key, &[], &format!("{MODELS_URL}{model}"))?;
    match code.as_str() {
        "200" => Ok(Check::Ok(format!("La chiave funziona e il modello «{model}» è disponibile."))),
        "404" => Ok(Check::Warn(format!("La chiave funziona, ma Gemini non elenca «{model}»: se la trascrizione non parte, controlla il nome del modello."))),
        "400" | "401" | "403" => Err(format!("chiave rifiutata da Gemini: {}", error_message(&body))),
        _ => Err(format!("Gemini ha risposto {code}: {}", error_message(&body))),
    }
}

fn error_message(body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| v["error"]["message"].as_str().map(String::from));
    msg.unwrap_or_else(|| body.trim().chars().take(160).collect())
}

fn speaker_index(label: &str) -> u32 {
    label.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0)
}

/// Le frasi con la loro voce, dalle annotazioni `word_info`; frasi consecutive della stessa voce
/// si uniscono. Senza annotazioni resta il testo intero, attribuito alla voce 0.
pub fn parse_utterances(json: &str) -> Result<Vec<Utterance>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("risposta di Gemini non valida: {e}"))?;
    if let Some(e) = v.get("error") {
        return Err(e["message"].as_str().unwrap_or("errore di Gemini").to_string());
    }
    let mut out: Vec<Utterance> = Vec::new();
    let mut plain = String::new();
    for step in v["steps"].as_array().into_iter().flatten() {
        for content in step["content"].as_array().into_iter().flatten() {
            plain.push_str(content["text"].as_str().unwrap_or(""));
            for a in content["annotations"].as_array().into_iter().flatten() {
                let text = a["text"].as_str().unwrap_or("").trim();
                if a["type"] != "word_info" || text.is_empty() {
                    continue;
                }
                let speaker = speaker_index(a["speaker"].as_str().unwrap_or("0"));
                match out.last_mut() {
                    Some(last) if last.speaker == speaker => {
                        last.text.push(' ');
                        last.text.push_str(text);
                    }
                    _ => out.push(Utterance { speaker, text: text.to_string() }),
                }
            }
        }
    }
    if out.is_empty() && !plain.trim().is_empty() {
        out.push(Utterance { speaker: 0, text: plain.trim().to_string() });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const REAL: &str = r#"{"id":"x","status":"completed","steps":[{"content":[{"text":"Buongiorno. Allora, gli ordini arrivano. E poi?","annotations":[
        {"start_index":0,"end_index":11,"text":"Buongiorno.","speaker":"spk:0","type":"word_info"},
        {"start_index":12,"end_index":33,"text":"Allora, gli ordini arrivano.","speaker":"spk:1","type":"word_info"},
        {"start_index":34,"end_index":40,"text":"E poi?","speaker":"spk:0","type":"word_info"}],"type":"text"}],"type":"model_output"}]}"#;

    #[test]
    fn utterances_keep_who_said_them() {
        let u = parse_utterances(REAL).unwrap();
        assert_eq!(u.iter().map(|x| x.speaker).collect::<Vec<_>>(), [0, 1, 0]);
        assert_eq!(u[1].text, "Allora, gli ordini arrivano.");
    }

    #[test]
    fn consecutive_sentences_of_the_same_voice_become_one_turn() {
        let j = r#"{"steps":[{"content":[{"text":"x","annotations":[
            {"text":"Prima frase.","speaker":"spk:1","type":"word_info"},
            {"text":"Seconda frase.","speaker":"spk:1","type":"word_info"},
            {"text":"Risposta.","speaker":"spk:0","type":"word_info"}]}]}]}"#;
        let u = parse_utterances(j).unwrap();
        assert_eq!(u.len(), 2);
        assert_eq!(u[0], Utterance { speaker: 1, text: "Prima frase. Seconda frase.".into() });
    }

    #[test]
    fn without_annotations_the_whole_text_is_one_utterance_and_silence_is_empty() {
        let u = parse_utterances(r#"{"steps":[{"content":[{"text":"  Solo testo  "}]}]}"#).unwrap();
        assert_eq!(u, [Utterance { speaker: 0, text: "Solo testo".into() }]);
        assert!(parse_utterances(r#"{"steps":[{"content":[{"text":""}]}]}"#).unwrap().is_empty());
    }

    #[test]
    fn an_error_reply_or_garbage_is_an_error_not_a_panic() {
        assert!(parse_utterances(r#"{"error":{"message":"quota esaurita"}}"#).unwrap_err().contains("quota"));
        assert!(parse_utterances("<html>").is_err());
    }

    #[test]
    fn speaker_labels_with_a_colon_or_an_underscore_both_work() {
        assert_eq!(speaker_index("spk:3"), 3);
        assert_eq!(speaker_index("spk_2"), 2);
        assert_eq!(speaker_index("?"), 0);
    }

    #[test]
    fn the_request_asks_for_the_chosen_model_in_the_chosen_language_with_speakers() {
        let b = request_body(b"abc", "gemini-3.5-transcribe", "it-IT");
        assert_eq!(b["model"], "gemini-3.5-transcribe");
        assert_eq!(b["input"][0]["mime_type"], "audio/ogg");
        assert_eq!(b["input"][0]["data"], "YWJj");
        let c = &b["generation_config"]["transcription_config"];
        assert_eq!(c["language_codes"][0], "it-IT");
        assert_eq!(c["mode"]["diarization_mode"], "speaker");
        assert_eq!(request_body(b"", "altro-modello", "en-US")["model"], "altro-modello");
    }

    /// Un finto curl: stampa una risposta e il codice HTTP, e salva ciò che riceve su stdin e
    /// l'indirizzo che chiede.
    fn fake_curl(dir: &Path, name: &str, reply: &str, code: &str) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{name}.sh"));
        std::fs::write(&path, format!("#!/bin/sh\nfor a; do last=\"$a\"; done\necho \"$last\" > \"{0}/url.txt\"\ncat > \"{0}/stdin.txt\"\necho '{1}'\necho '{2}'\n", dir.display(), reply, code)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn the_key_goes_through_stdin_and_a_failure_names_the_status_without_the_key() {
        let dir = std::env::temp_dir().join(format!("rr-curl-{}", std::process::id()));
        let req = dir.join("r.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&req, "{}").unwrap();

        let bad = fake_curl(&dir, "bad", r#"{"error":{"message":"API key not valid"}}"#, "400");
        let err = post_with(&bad, "SEGRETA123", &req).unwrap_err();
        assert!(err.contains("400") && err.contains("API key not valid"), "{err}");
        assert!(!err.contains("SEGRETA123"));
        let sent = std::fs::read_to_string(dir.join("stdin.txt")).unwrap();
        assert!(sent.contains("x-goog-api-key: SEGRETA123"), "la chiave arriva a curl dalla sua configurazione");

        let good = fake_curl(&dir, "good", r#"{"steps":[]}"#, "200");
        assert_eq!(post_with(&good, "k", &req).unwrap(), r#"{"steps":[]}"#);
        assert!(post_with(&good, "k\"; evil", &req).is_err(), "una chiave con virgolette non entra nella configurazione");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn checking_the_key_asks_for_the_model_and_tells_a_bad_key_from_an_unknown_model() {
        let dir = std::env::temp_dir().join(format!("rr-check-{}", std::process::id()));
        let key = "AQ.chiave-abbastanza-lunga-123";
        let ok = fake_curl(&dir, "ok", r#"{"name":"models/gemini-3.5-transcribe"}"#, "200");
        assert!(matches!(check_with(&ok, key, "gemini-3.5-transcribe"), Ok(Check::Ok(_))));
        assert!(std::fs::read_to_string(dir.join("url.txt")).unwrap().trim().ends_with("/v1beta/models/gemini-3.5-transcribe"));
        assert!(std::fs::read_to_string(dir.join("stdin.txt")).unwrap().contains(key));

        let missing = fake_curl(&dir, "missing", r#"{"error":{"message":"not found"}}"#, "404");
        assert!(matches!(check_with(&missing, key, "models/inventato"), Ok(Check::Warn(_))), "la chiave va, il modello no");
        let refused = fake_curl(&dir, "refused", r#"{"error":{"message":"API key not valid. Please pass a valid API key."}}"#, "400");
        assert!(check_with(&refused, key, "gemini-3.5-transcribe").unwrap_err().contains("API key not valid"));
        assert!(check_with(&ok, "corta", "gemini-3.5-transcribe").is_err(), "una chiave malformata non parte nemmeno");
        assert!(check_with(&ok, key, "../segreti").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Trascrive un dialogo vero con la chiave vera:
    /// `RR_TEST_AUDIO=file.ogg cargo test --release live_gemini -- --ignored --nocapture`
    /// (chiave dalle impostazioni o da `GEMINI_API_KEY`).
    #[test]
    #[ignore]
    fn live_gemini_separates_two_voices() {
        let key = crate::settings::active_key(crate::settings::config_dir().as_deref()).0.expect("serve una chiave Gemini");
        let audio = PathBuf::from(std::env::var("RR_TEST_AUDIO").expect("serve RR_TEST_AUDIO"));
        let dir = std::env::temp_dir().join(format!("rr-live-tx-{}", std::process::id()));
        let job = Job { area: 0, window: "w".into(), files: vec![audio], work_dir: dir.clone(), key, model: crate::settings::DEFAULT_TRANSCRIBE_MODEL.into(), lang: "it-IT".into() };
        let u = run(&job).expect("trascrizione");
        for x in &u {
            println!("voce {} | {}", x.speaker, x.text);
        }
        assert!(u.iter().any(|x| x.speaker == 0) && u.iter().any(|x| x.speaker == 1), "due voci distinte");
        let _ = std::fs::remove_dir_all(dir);
    }
}
