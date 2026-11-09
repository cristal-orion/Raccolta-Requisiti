pub mod board;
pub mod home;
pub mod settings;
pub mod side;

/// Il passo da 16 punti che `rune::tok::space` non ha.
pub const S4: f32 = 16.0;

use rune::tok::{self, radius, space, type_};
use rune::{Cx, El, Key, col, el, oklch, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Switch, TextInput};

use crate::state::{AgentStatus, ExportStatus, Rec, State};

fn red() -> rune::Color {
    oklch(0.66, 0.23, 27.0)
}

/// `754` → `12:34`, `3725` → `1:02:05`.
pub fn fmt_time(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

/// Registra / Ferma / Riprendi: sempre in barra, con l'indicatore rosso mentre si registra.
fn rec_control(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let toggle = |s: &mut State| s.toggle_recording();
    match s.rec {
        Rec::Idle => Button::new("● Registra").variant(ButtonVariant::Danger).on_press(toggle).build(k).key("rec"),
        Rec::Live => {
            let label = if s.recording_here() {
                format!("REC {}", fmt_time(s.rec_secs()))
            } else {
                format!("REC su «{}»", s.rec_area_name().unwrap_or_default())
            };
            row().key("rec-live").gap(space::S2).padding(space::S2).center_y().radius(99.0).fill(red().alpha(0.16)).border(1.0, red().alpha(0.6))
                .child(el().size(10.0, 10.0).radius(5.0).fill(red()))
                .child(text(label).font(type_::LABEL).color(t.text))
                .child(Button::new("Ferma").variant(ButtonVariant::Outline).on_press(toggle).build(k).key("rec"))
        }
        Rec::Paused => row().key("rec-paused").gap(space::S2).padding(space::S2).center_y().radius(99.0).fill(t.card)
            .child(el().size(10.0, 10.0).radius(5.0).border(2.0, t.text_secondary))
            .child(text(format!("In pausa {}", fmt_time(s.rec_secs()))).font(type_::LABEL).color(t.text_secondary))
            .child(Button::new("Riprendi").variant(ButtonVariant::Submit).on_press(toggle).build(k).key("rec")),
    }
}

fn topbar(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let pending: usize = s.areas().iter().map(|a| a.pending).sum();
    let pill = row().key("pending").padding(space::S2).gap(space::S1).radius(99.0).center_y()
        .fill(if pending > 0 { board::amber().alpha(0.18) } else { t.card })
        .child(el().size(8.0, 8.0).radius(4.0).fill(if pending > 0 { board::amber() } else { t.text_secondary }))
        .child(text(if pending == 1 { "1 in sospeso".to_string() } else { format!("{pending} in sospeso") }).font(type_::LABEL).color(t.text));
    row().w_full().padding(space::S3).gap(space::S3).center_y().fill(t.bg).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(Button::new("Colloqui").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.open_home()).build(k).key("home"))
        .child(TextInput::new(&s.client_edit).width(Some(200.0)).placeholder("Cliente").on_edit(|s: &mut State| &mut s.client_edit).build(k).key("client"))
        .child(rec_control(s, k, dark))
        .child(el().grow(1.0))
        .child(pill)
        .child(
            Button::new(if s.export == ExportStatus::Running { "Preparo il PDF…" } else { "Esporta PDF" })
                .variant(ButtonVariant::Outline)
                .disabled(!s.can_export())
                .on_press(|s: &mut State| s.start_export())
                .build(k)
                .key("export"),
        )
        .child(Switch::new(s.auto).label("Auto").on_toggle(|s: &mut State| s.auto = !s.auto).build(k).key("auto"))
        .child(
            Button::new(if s.agent == AgentStatus::Running { "In corso…" } else { "Aggiorna ora" })
                .variant(ButtonVariant::Submit)
                .disabled(!s.can_run())
                .on_press(|s: &mut State| s.start_round())
                .build(k)
                .key("run"),
        )
        .child(Button::new("Impostazioni").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.open_settings()).build(k).key("settings"))
}

/// Lo stato dell'agente o, se c'è, l'errore dell'audio, che conta di più.
fn status(s: &State, dark: bool) -> (String, rune::Color) {
    let t = tok::theme(dark);
    if let Some(e) = &s.audio_error {
        return (e.clone(), red());
    }
    if let Some(e) = &s.transcribe_error {
        return (format!("Trascrizione: {e}"), board::amber());
    }
    match &s.export {
        ExportStatus::Running => return ("Preparo il verbale in PDF: GPT lo scrive, ci vuole qualche minuto…".to_string(), tok::accent(0, dark)),
        ExportStatus::Failed(e) => return (format!("Esportazione non riuscita: {e}"), red()),
        ExportStatus::Done(p) => return (format!("PDF salvato: {}", p.display()), tok::accent(0, dark)),
        ExportStatus::Idle => {}
    }
    if s.transcription_off() && (s.rec != Rec::Idle || s.pending_audio_secs() > 0.0) {
        return ("Trascrizione spenta: manca la chiave di Gemini (Impostazioni)".to_string(), board::amber());
    }
    match &s.agent {
        AgentStatus::Running => {
            let elsewhere = s.running_area.filter(|id| *id != s.session.id).and_then(|id| s.area_name(id));
            (elsewhere.map_or("L'agente sta pensando…".to_string(), |n| format!("L'agente lavora su «{n}»…")), tok::accent(0, dark))
        }
        AgentStatus::Failed(e) => (format!("Errore: {e}"), board::amber()),
        AgentStatus::Idle if s.transcribing_now() => ("Trascrivo l'audio…".to_string(), tok::accent(0, dark)),
        AgentStatus::Idle if s.fresh && s.auto => ("Nuovo materiale: aggiorno tra poco".to_string(), t.text_secondary),
        AgentStatus::Idle if s.fresh => ("Nuovo materiale da analizzare".to_string(), t.text_secondary),
        AgentStatus::Idle => ("Agente in ascolto".to_string(), t.text_secondary),
    }
}

/// Le aree di lavoro: una per argomento, con il suo interlocutore.
fn areas_bar(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let accent = tok::accent(0, dark);
    let mut bar = row().w_full().padding(space::S2).gap(space::S2).center_y().fill(t.bg).border_sides([0.0, 0.0, 1.0, 0.0], t.separator);
    for a in s.areas() {
        if a.index == s.active && s.renaming {
            bar = bar
                .child(
                    TextInput::new(&s.name_edit)
                        .width(Some(200.0))
                        .placeholder("Nome dell'area")
                        .on_edit(|s: &mut State| &mut s.name_edit)
                        .on_key(|s: &mut State, ev: &rune::KeyEvent, _tx: &mut rune::Tx| {
                            if ev.key == Key::Enter {
                                s.finish_rename();
                                true
                            } else {
                                false
                            }
                        })
                        .build(k)
                        .key("area-name"),
                )
                .child(Button::new("OK").variant(ButtonVariant::Submit).on_press(|s: &mut State| s.finish_rename()).build(k).key("area-name-ok"));
            continue;
        }
        let active = a.index == s.active;
        let i = a.index;
        let mut chip = row().key(format!("area:{i}")).role("button").label(format!("Area {}", a.name)).focusable(true)
            .padding(space::S2).gap(space::S2).center_y().radius(radius::MD)
            .fill(if active { accent.alpha(0.18) } else { t.card })
            .border(1.0, if active { accent } else { t.card_border });
        if a.recording {
            chip = chip.child(el().size(8.0, 8.0).radius(4.0).fill(red()));
        }
        chip = chip.child(text(a.name.clone()).font(type_::BODY).color(t.text));
        if a.pending > 0 {
            chip = chip.child(row().padding(2.0).radius(99.0).fill(board::amber().alpha(0.25)).child(text(format!(" {} ", a.pending)).font(type_::LABEL).color(board::amber())));
        }
        bar = bar.child(chip.on_press(move |s: &mut State| s.switch_area(i)));
    }
    bar = bar.child(Button::new("+ Nuova area").variant(ButtonVariant::Outline).on_press(|s: &mut State| s.add_area()).build(k).key("area-new"));
    if !s.renaming {
        bar = bar.child(Button::new("Rinomina").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.start_rename()).build(k).key("area-rename"));
    }
    let (msg, tint) = status(s, dark);
    bar.child(el().grow(1.0)).child(text(msg).font(type_::LABEL).color(tint).wrap().min_w(0.0))
}

/// Solo il diagramma, per disegnarlo in un'immagine da mettere nel PDF.
fn export_diagram_view(s: &State, dark: bool) -> El {
    let mut style = rune_mermaid::DiagramStyle::neutral(dark).min_scale(0.0);
    style.max_width = 1700.0;
    let t = tok::theme(dark);
    let card = rune_mermaid::render(&s.session.mermaid, &style).unwrap_or_else(|| text("Diagramma non disegnabile").font(type_::BODY).color(t.text));
    col().w_full().h_full().fill(t.bg).padding(8.0).child(card)
}

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = cx.system_dark();
    if s.export_only {
        return export_diagram_view(s, dark);
    }
    let k = &Kit::neutral(dark);
    let t = tok::theme(dark);
    let body = row().w_full().grow(1.0).min_h(0.0)
        .child(side::transcript(s, k, dark))
        .child(board::view(s, cx, k, dark))
        .child(side::right(s, k, dark));
    let base = if s.home.is_some() {
        col().w_full().h_full().fill(t.bg).child(home::view(s, k, dark))
    } else {
        col().w_full().h_full().fill(t.bg)
            .child(topbar(s, k, dark))
            .child(areas_bar(s, k, dark))
            .child(body)
            .on_drop_files(|s: &mut State, files, _| s.add_files(files))
    };
    let mut layer = Vec::new();
    let window_h = cx.layout_size("kit-root").map_or(900.0, |[_, h]| h);
    settings::dialog(s, k, dark, window_h, &mut layer);
    rune_kit::root(base, layer)
}
