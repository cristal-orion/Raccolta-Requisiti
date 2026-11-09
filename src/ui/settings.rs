//! Il pannello delle impostazioni: agente, trascrizione e microfono.

use rune::tok::{self, space, type_};
use rune::{El, ScrollSpec, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, Kit, SegmentedControl, Select, SelectAppearance, Switch, TextInput};

use crate::settings::{EFFORTS, KeySource};
use crate::settings::VOLUME_MAX;
use crate::state::{KeyCheck, LEVEL_H, LEVEL_W, Picker, Rec, SettingsForm, State};
use crate::ui::board::amber;

const WIDTH: f32 = 560.0;
/// La larghezza utile dentro il pannello (16 punti di margine per lato).
const INNER: f32 = WIDTH - 2.0 * 16.0;
const EFFORT_LABELS: [&str; 3] = ["Rapido", "Medio", "Approfondito"];

fn red() -> rune::Color {
    rune::oklch(0.66, 0.23, 27.0)
}

fn green() -> rune::Color {
    rune::oklch(0.74, 0.15, 150.0)
}

fn section(title: &str, dark: bool) -> El {
    let t = tok::theme(dark);
    col().w_full().gap(space::S2).pt(space::S2).border_sides([1.0, 0.0, 0.0, 0.0], t.separator)
        .child(el().h(space::S1))
        .child(text(title.to_string()).font(type_::TITLE).color(t.text))
}

fn field(label: &str, control: El, dark: bool) -> El {
    let t = tok::theme(dark);
    col().w_full().gap(space::S1).child(text(label.to_string()).font(type_::LABEL).color(t.text_secondary)).child(control)
}

/// Un campo largo `w`, per metterne due sulla stessa riga.
fn field_w(label: &str, control: El, w: f32, dark: bool) -> El {
    let t = tok::theme(dark);
    col().w(w).gap(space::S1).child(text(label.to_string()).font(type_::LABEL).color(t.text_secondary)).child(control)
}

fn note(msg: impl Into<String>, color: rune::Color) -> El {
    text(msg.into()).font(type_::LABEL).color(color).wrap().w_full()
}

fn agent(f: &SettingsForm, k: &Kit, dark: bool, layer: &mut Vec<El>) -> El {
    let t = tok::theme(dark);
    let (names, current) = f.model_options();
    let model = Select::new("set-model", &names)
        .current(Some(&current))
        .appearance(SelectAppearance::Field)
        .width(INNER)
        .label("Modello dell'agente")
        .open(f.picker == Some(Picker::Model))
        .on_toggle(|s: &mut State| s.toggle_picker(Picker::Model))
        .on_close(|s: &mut State| s.close_picker())
        .on_choose(|s: &mut State, v: &str, _| {
            if let Some(f) = &mut s.form {
                f.choose_model(v);
            }
        })
        .build(k, layer);
    let effort = SegmentedControl::new(&EFFORT_LABELS.map(String::from), EFFORTS.iter().position(|e| *e == f.agent_effort))
        .width(Some(INNER))
        .on_pick(|s: &mut State, i, _| {
            if let Some(f) = &mut s.form {
                f.agent_effort = EFFORTS[i.min(EFFORTS.len() - 1)].to_string();
            }
        })
        .build(k);
    let fast = Switch::new(f.agent_fast)
        .label("Risposte veloci (servizio prioritario, consuma 1,5× del piano)")
        .on_toggle(|s: &mut State| {
            if let Some(f) = &mut s.form {
                f.agent_fast = !f.agent_fast;
            }
        })
        .build(k)
        .key("set-fast");
    section("Agente (lavagna, domande, verbale)", dark)
        .child(field("Modello (Codex)", model, dark))
        .child(field("Ragionamento", effort, dark))
        .child(fast)
        .child(note(format!("Accesso: {}. L'agente usa la CLI codex, senza chiavi da inserire qui.", f.codex), t.text_secondary))
}

fn transcription(s: &State, f: &SettingsForm, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let (model_w, lang_w) = (INNER - 140.0 - space::S3, 140.0);
    let model = TextInput::new(&f.transcribe_model)
        .width(Some(model_w))
        .placeholder("gemini-3.5-transcribe")
        .on_edit(|s: &mut State| &mut s.form.as_mut().expect("pannello aperto").transcribe_model)
        .build(k)
        .key("set-tx-model");
    let lang = TextInput::new(&f.transcribe_lang)
        .width(Some(lang_w))
        .placeholder("it-IT")
        .on_edit(|s: &mut State| &mut s.form.as_mut().expect("pannello aperto").transcribe_lang)
        .build(k)
        .key("set-tx-lang");
    let in_use = match (s.key_source, s.key_tail()) {
        (KeySource::Saved, Some(tail)) => (format!("Chiave in uso: salvata qui ({tail})."), t.text_secondary),
        (KeySource::Env, Some(tail)) => (format!("Chiave in uso: GEMINI_API_KEY dall'ambiente ({tail}). Una chiave salvata qui ha la precedenza."), t.text_secondary),
        _ => ("Nessuna chiave: l'audio si registra ma non diventa testo.".to_string(), amber()),
    };
    let key = TextInput::new(&f.new_key)
        .width(Some(INNER - 96.0 - space::S2))
        .placeholder(if s.key_source == KeySource::None { "Incolla la chiave API di Gemini" } else { "Incolla una chiave nuova per sostituirla" })
        .secure()
        .on_edit(|s: &mut State| &mut s.form.as_mut().expect("pannello aperto").new_key)
        .build(k)
        .key("set-key");
    let checking = f.check == KeyCheck::Running;
    let check = Button::new(if checking { "Provo…" } else { "Prova" })
        .variant(ButtonVariant::Outline)
        .disabled(checking)
        .on_press(|s: &mut State| s.check_key())
        .build(k)
        .key("set-key-check")
        .w(96.0);
    let mut out = section("Trascrizione (Gemini)", dark)
        .child(row().w_full().gap(space::S3).child(field_w("Modello", model, model_w, dark)).child(field_w("Lingua (es. it-IT)", lang, lang_w, dark)))
        .child(field("Chiave API", row().w_full().gap(space::S2).center_y().child(key).child(check), dark))
        .child(note(in_use.0, in_use.1));
    let result = match &f.check {
        KeyCheck::Idle | KeyCheck::Running => None,
        KeyCheck::Ok(m) => Some((m.clone(), green())),
        KeyCheck::Warn(m) => Some((m.clone(), amber())),
        KeyCheck::Failed(e) => Some((format!("Non va: {e}"), red())),
    };
    if let Some((m, c)) = result {
        out = out.child(note(m, c));
    }
    out.child(note("Il modello deve saper separare le voci. La chiave si salva in ~/.config/raccolta-requisiti, leggibile solo da te.", t.text_secondary))
}

fn audio(s: &State, f: &SettingsForm, k: &Kit, dark: bool, layer: &mut Vec<El>) -> El {
    let t = tok::theme(dark);
    let (labels, current) = f.source_options();
    let pick = Select::new("set-source", &labels)
        .current(Some(&current))
        .appearance(SelectAppearance::Field)
        .width(INNER - 96.0 - space::S2)
        .label("Microfono")
        .open(f.picker == Some(Picker::Source))
        .on_toggle(|s: &mut State| s.toggle_picker(Picker::Source))
        .on_close(|s: &mut State| s.close_picker())
        .on_choose(|s: &mut State, v: &str, _| s.choose_source(v))
        .build(k, layer);
    let reload = Button::new("Aggiorna")
        .variant(ButtonVariant::Outline)
        .on_press(|s: &mut State| s.reload_mics())
        .build(k)
        .key("set-source-reload")
        .w(96.0);
    let mut out = section("Audio", dark).child(field("Microfono", row().w_full().gap(space::S2).center_y().child(pick).child(reload), dark));
    if let Some(e) = &f.sources_error {
        out = out.child(note(format!("{e}: resta il microfono predefinito."), amber()));
    }
    if f.chosen_source_muted() {
        out = out.child(
            row().w_full().gap(space::S2).center_y()
                .child(note("Questo microfono è silenziato nel sistema: registrerebbe silenzio.", amber()).grow(1.0).min_w(0.0))
                .child(Button::new("Riattiva").variant(ButtonVariant::Outline).on_press(|s: &mut State| s.unmute_mic()).build(k).key("set-unmute")),
        );
    }
    out = out.child(field("Volume in ingresso", volume_row(f, k, dark), dark)).child(level_bar(f, dark));
    if let Some(e) = f.mic.error.as_ref().or(f.mic.meter_error.as_ref()) {
        out = out.child(note(e.clone(), amber()));
    }
    out = out.child(note("Parla con voce normale: la barra deve stare nel verde e arrivare al giallo sui picchi; se va sul rosso, abbassa. È il volume del sistema: vale anche per le altre app. «Annulla» lo riporta com'era.", t.text_secondary));
    let tip = if s.rec == Rec::Live {
        "Collegato un microfono esterno? Premi «Aggiorna». Il cambio vale subito: la registrazione prosegue dal nuovo microfono."
    } else {
        "Collegato un microfono esterno? Premi «Aggiorna» e sceglilo. Se manca quando registri, si usa il predefinito."
    };
    out.child(note(tip, t.text_secondary))
}

const VOLUME_W: f32 = INNER - 2.0 * 36.0 - 56.0 - 3.0 * space::S2;
const KNOB: f32 = 16.0;

/// Il cursore del volume d'ingresso (0–150%), con − e + a passi del 5%.
fn volume_row(f: &SettingsForm, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let accent = tok::accent(0, dark);
    let pct = f.mic.volume;
    let frac = pct.map_or(0.0, |p| p as f32 / VOLUME_MAX as f32);
    let x = frac * (VOLUME_W - KNOB);
    // La tacca del 100%: oltre, PipeWire amplifica in software.
    let unity = 100.0 / VOLUME_MAX as f32 * (VOLUME_W - KNOB) + KNOB * 0.5;
    let slider = el().key("set-volume").role("slider").label("Volume in ingresso").focusable(true).size(VOLUME_W, 24.0).cursor(rune::Cursor::Pointer)
        .child(el().absolute(0.0, 10.0).size(VOLUME_W, 4.0).radius(2.0).fill(t.control_track))
        .child(el().absolute(0.0, 10.0).size(x + KNOB * 0.5, 4.0).radius(2.0).fill(accent))
        .child(el().absolute(unity - 1.0, 5.0).size(2.0, 14.0).radius(1.0).fill(t.text_secondary.alpha(0.6)).inert())
        .child(el().absolute(x, 4.0).size(KNOB, KNOB).radius(KNOB * 0.5).fill(t.knob).border(1.0, t.card_border))
        .on_drag(|s: &mut State, d: &mut rune::Drag, _tx: &mut rune::Tx| {
            if d.w > 0.0 {
                let frac = ((d.x - KNOB * 0.5) / (d.w - KNOB)).clamp(0.0, 1.0);
                s.set_mic_volume((frac * VOLUME_MAX as f32).round() as u32, d.phase == rune::Phase::Up);
            }
        });
    let step = move |delta: i32| move |s: &mut State| {
        let now = s.form.as_ref().and_then(|f| f.mic.volume).unwrap_or(100) as i32;
        // Al multiplo di 5 più vicino nella direzione del pulsante.
        let next = if delta > 0 { (now / 5 + 1) * 5 } else { ((now + 4) / 5 - 1) * 5 };
        s.set_mic_volume(next.clamp(0, VOLUME_MAX as i32) as u32, true);
    };
    let label = match pct {
        Some(p) => format!("{p}%"),
        None => "—".to_string(),
    };
    row().w_full().gap(space::S2).center_y()
        .child(Button::new("−").variant(ButtonVariant::Outline).disabled(pct.is_none()).on_press(step(-5)).build(k).key("set-volume-down").w(36.0))
        .child(slider)
        .child(Button::new("+").variant(ButtonVariant::Outline).disabled(pct.is_none()).on_press(step(5)).build(k).key("set-volume-up").w(36.0))
        .child(text(label).font(type_::BODY).color(if pct.is_some_and(|p| p > 100) { amber() } else { t.text }).w(56.0))
}

/// Il livello del microfono dal vivo: la barra la muove il thread che ascolta.
fn level_bar(f: &SettingsForm, dark: bool) -> El {
    let t = tok::theme(dark);
    let track = el().key("set-level").w(LEVEL_W).h(LEVEL_H).radius(LEVEL_H * 0.5).fill(t.control_track)
        .child(el().absolute(0.0, 0.0).h(LEVEL_H).w(0.0).radius(LEVEL_H * 0.5).live_rect(&f.mic.level_rect).live_fill(&f.mic.level_color).inert());
    field("Livello del microfono (parla per provarlo)", track, dark)
}

/// Il pannello, se è aperto: va nello strato sopra l'app. `window_h` è l'altezza della finestra:
/// il contenuto scorre solo se non ci sta.
pub fn dialog(s: &State, k: &Kit, dark: bool, window_h: f32, layer: &mut Vec<El>) {
    let Some(f) = &s.form else { return };
    // I menu a tendina si costruiscono prima del pannello: il loro elenco sta sopra di lui.
    let agent = agent(f, k, dark, layer);
    let audio = audio(s, f, k, dark, layer);
    let body = col().w_full().padding(16.0).pt(0.0).gap(space::S3)
        .child(agent)
        .child(transcription(s, f, k, dark))
        .child(audio);
    let mut footer = row().key("set-footer").w_full().padding(16.0).gap(space::S2).center_y();
    if let Some(e) = &f.error {
        footer = footer.child(note(e.clone(), red()).grow(1.0).min_w(0.0));
    } else {
        footer = footer.child(el().grow(1.0));
    }
    footer = footer
        .child(Button::new("Annulla").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.close_settings()).build(k).key("set-cancel"))
        .child(Button::new("Salva").variant(ButtonVariant::Submit).on_press(|s: &mut State| s.save_settings()).build(k).key("set-save"));
    Dialog::new("settings", "Impostazioni")
        .open(true)
        .description("Modelli AI, chiave di Gemini e microfono. Valgono per tutte le sessioni.")
        .width(WIDTH)
        .fit()
        .click_outside(false)
        .on_close(|s: &mut State| s.close_settings())
        .child(col().key("set-scroll").w_full().max_h((window_h - 230.0).max(220.0)).scroll(ScrollSpec { stick_bottom: false }).child(body))
        .child(footer)
        .build(k, layer);
}
