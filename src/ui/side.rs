//! Pannelli laterali: trascrizione a sinistra; note, allegati e domande a destra.

use rune::tok::{self, radius, space, type_};
use rune::{El, Key, ScrollSpec, SurfaceFit, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Kit, SegmentedControl, TextField, TextInput};

use crate::model::{AttachmentKind, QStatus, Speaker};
use crate::state::{State, Tab};
use crate::ui::board::amber;

pub const LEFT_W: f32 = 300.0;
pub const RIGHT_W: f32 = 372.0;
/// La striscia a cui si riduce il pannello di destra, e lo spazio del pulsante che lo riduce.
pub const RAIL_W: f32 = 52.0;
const COLLAPSE_W: f32 = 44.0;

pub fn transcript(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let mut list = col().w_full().padding(space::S3).gap(space::S3);
    if s.session.turns.is_empty() {
        list = list.child(text("Qui compaiono le battute del colloquio: premi «Registra» e si trascrivono da sole, separate per voce. Puoi anche aggiungerle a mano.").font(type_::LABEL).color(t.text_secondary).wrap());
    }
    for (i, turn) in s.session.turns.iter().enumerate() {
        // Sopra ogni blocco che viene dall'audio: da dove arriva e come scambiare le voci.
        if let Some(src) = &turn.source {
            if i == 0 || s.session.turns[i - 1].source.as_ref() != Some(src) {
                let owned = src.clone();
                let swapped = s.session.voice_swaps.contains(src);
                list = list.child(
                    row().key(format!("source:{i}")).w_full().gap(space::S2).center_y()
                        .child(text(if swapped { "Dall'audio · voci scambiate" } else { "Dall'audio" }).font(type_::LABEL).color(t.text_secondary).grow(1.0))
                        .child(Button::new("Scambia voci").variant(ButtonVariant::Ghost).on_press(move |s: &mut State| s.swap_voices(&owned)).build(k).key(format!("swap:{i}"))),
                );
            }
        }
        let me = turn.speaker == Speaker::Io;
        let tint = if me { tok::accent(0, dark) } else { rune::oklch(0.78, 0.12, 165.0) };
        list = list.child(
            col().key(format!("turn:{i}")).w_full().gap(2.0)
                .child(text(s.session.speaker_name(turn.speaker)).font(type_::LABEL).color(tint))
                .child(text(turn.text.clone()).font(type_::BODY).color(t.text).wrap()),
        );
    }
    let who: String = s.session.speaker_name(Speaker::Cliente).chars().take(16).collect();
    let pick = SegmentedControl::new(&["Io".to_string(), who], Some(if s.speaker == Speaker::Io { 0 } else { 1 }))
        .fit()
        .on_pick(|s: &mut State, i, _| s.speaker = if i == 0 { Speaker::Io } else { Speaker::Cliente })
        .build(k);
    let input = TextInput::new(&s.turn_edit)
        .width(None)
        .placeholder("Cosa è stato detto…")
        .on_edit(|s: &mut State| &mut s.turn_edit)
        .on_key(|s: &mut State, ev: &rune::KeyEvent, _| {
            if ev.key == Key::Enter && !ev.mods.shift {
                s.submit_turn();
                true
            } else {
                false
            }
        })
        .build(k)
        .grow(1.0);
    let composer = col().w_full().padding(space::S3).gap(space::S2).border_sides([1.0, 0.0, 0.0, 0.0], t.separator)
        .child(pick)
        .child(row().w_full().gap(space::S2).child(input).child(Button::new("Aggiungi").variant(ButtonVariant::Submit).on_press(|s: &mut State| s.submit_turn()).build(k).key("add-turn")));

    col().w(LEFT_W).h_full().min_h(0.0).fill(t.bg).border_sides([0.0, 1.0, 0.0, 0.0], t.separator)
        .child(col().w_full().padding(space::S3).gap(space::S2)
            .child(text("Trascrizione").font(type_::TITLE).color(t.text))
            .child(TextInput::new(&s.interlocutor_edit).width(Some(LEFT_W - 2.0 * space::S3)).placeholder("Interlocutore (es. Marco, IT)")
                .on_edit(|s: &mut State| &mut s.interlocutor_edit).build(k).key("interlocutor")))
        .child(col().grow(1.0).min_h(0.0).w_full().shrink().scroll(ScrollSpec { stick_bottom: true }).child(list))
        .child(composer)
}

fn notes(s: &State, k: &Kit) -> El {
    TextField::new("Note", &s.notes_edit)
        .hide_label()
        .multiline()
        .placeholder("Le tue note: l'agente le legge insieme alla trascrizione.")
        .on_edit(|s: &mut State| &mut s.notes_edit)
        .build(k)
        .w_full()
        .min_h(260.0)
        .grow(1.0)
}

fn files(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let accent = tok::accent(0, dark);
    let zone = col().key("dropzone").w_full().padding(crate::ui::S4).gap(space::S1).radius(radius::LG).center()
        .fill(if s.drop_over { accent.alpha(0.16) } else { t.card })
        .border(if s.drop_over { 2.0 } else { 1.0 }, if s.drop_over { accent } else { t.card_border })
        .child(text("Trascina qui foto e documenti").font(type_::TITLE).color(t.text))
        .child(text("Immagini, PDF, testi, CSV: l'agente li legge").font(type_::LABEL).color(t.text_secondary))
        .child(Button::new("Scegli file…").variant(ButtonVariant::Outline).on_press_tx(|_: &mut State, tx: &mut rune::Tx| {
            let opts = rune::FileDialogOptions { title: "Aggiungi un allegato".into(), ..Default::default() };
            tx.open_file(opts, |s: &mut State, res, _| match res {
                Ok(Some(path)) => {
                    s.file_error = None;
                    s.add_files(&[path]);
                }
                Ok(None) => s.file_error = None,
                Err(e) => s.file_error = Some(format!("Il selettore dei file non si è aperto: {e}. Puoi sempre trascinare i file qui.")),
            });
        }).build(k).key("pick-file"))
        .on_drop_files(|s: &mut State, files, _| {
            s.drop_over = false;
            s.add_files(files);
        })
        .on_drop_hover(|s: &mut State, over, _| s.drop_over = over);

    let mut list = col().w_full().gap(space::S2);
    if let Some(e) = &s.file_error {
        list = list.child(text(e.clone()).font(type_::LABEL).color(amber()).wrap());
    }
    for (i, a) in s.session.attachments.iter().enumerate() {
        let thumb = match (a.kind, s.thumbs.get(&a.path)) {
            (AttachmentKind::Image, Some(img)) => el().size(52.0, 52.0).radius(radius::SM).pixels_fit(img.pixels(), SurfaceFit::Fill),
            _ => el().size(52.0, 52.0).radius(radius::SM).fill(t.control_track).center().child(text("DOC").font(type_::LABEL).color(t.text_secondary)),
        };
        let status = if a.summary.is_some() { "letto dall'agente" } else { "in attesa" };
        list = list.child(
            row().key(format!("file:{i}")).w_full().gap(space::S3).center_y()
                .child(thumb)
                .child(col().grow(1.0).min_w(0.0).gap(2.0)
                    .child(text(a.name.clone()).font(type_::BODY).color(t.text))
                    .child(text(a.summary.clone().unwrap_or_else(|| status.to_string())).font(type_::LABEL).color(t.text_secondary).wrap())),
        );
    }
    col().w_full().grow(1.0).min_h(0.0).gap(space::S3)
        .child(zone)
        .child(col().grow(1.0).min_h(0.0).w_full().shrink().scroll(ScrollSpec { stick_bottom: false }).child(list))
}

/// Il campo per rispondere dalla scheda Domande: Invio (o «Invia») manda la risposta all'agente.
fn side_answer(s: &State, k: &Kit, id: u32) -> El {
    let edit = s.side_answers.get(&id).cloned().unwrap_or_else(|| rune::TextEdit::new(""));
    let input = TextInput::new(&edit)
        .width(None)
        .placeholder("Rispondi…")
        .on_edit(move |s: &mut State| s.side_answer_edit(id))
        .on_key(move |s: &mut State, ev: &rune::KeyEvent, _| {
            if ev.key == Key::Enter && !ev.mods.shift {
                s.send_side_answer(id);
                true
            } else {
                false
            }
        })
        .build(k)
        .key(format!("side-answer:{id}"))
        .grow(1.0)
        .min_w(0.0);
    row().w_full().gap(space::S2).center_y()
        .child(input)
        .child(Button::new("Invia").variant(ButtonVariant::Submit).on_press(move |s: &mut State| s.send_side_answer(id)).build(k).key(format!("side-send:{id}")))
}

fn questions(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let mut list = col().w_full().gap(space::S3);
    if s.session.questions.is_empty() {
        list = list.child(text("Le domande dell'agente restano salvate qui, anche quelle chiuse e quelle a cui hai già risposto.").font(type_::LABEL).color(t.text_secondary).wrap());
    }
    for q in s.session.questions.iter().rev() {
        let id = q.id;
        let tint = match q.status {
            QStatus::Answered => t.text_secondary,
            _ => amber(),
        };
        let mut card = col().key(format!("q:{id}")).w_full().padding(space::S3).gap(space::S1).radius(radius::MD)
            .fill(t.card).border(1.0, if q.status == QStatus::Answered { t.card_border } else { amber().alpha(0.5) })
            .child(text(format!("{} · {}", q.status.label(), q.node.as_deref().map_or("generale".to_string(), |n| format!("nodo {n}")))).font(type_::LABEL).color(tint))
            .child(text(q.text.clone()).font(type_::BODY).color(t.text).wrap());
        if let Some(a) = &q.answer {
            card = card.child(text(format!("→ {a}")).font(type_::BODY).color(t.text_secondary).wrap());
        }
        if q.status != QStatus::Answered {
            card = card.child(side_answer(s, k, id));
        }
        if q.status == QStatus::Parked {
            card = card.child(Button::new("Riapri sulla lavagna").variant(ButtonVariant::Ghost).on_press(move |s: &mut State| s.reopen_question(id)).build(k).key(format!("reopen:{id}")));
        }
        list = list.child(card);
    }
    col().grow(1.0).min_h(0.0).w_full().shrink().scroll(ScrollSpec { stick_bottom: false }).child(list)
}

/// La striscia a cui si riduce il pannello di destra: riapre il pannello, e i contatori lo
/// riaprono sulla scheda giusta.
fn rail(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let pending = s.session.pending_count();
    let mut r = col().w(RAIL_W).h_full().min_h(0.0).padding(space::S2).gap(space::S2).items_center().fill(t.bg)
        .border_sides([0.0, 0.0, 0.0, 1.0], t.separator)
        .child(Button::new("«").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.toggle_right()).build(k).key("right-expand"));
    let badge = |key: &'static str, n: usize, tint: rune::Color, tab: Tab| {
        row().key(key).role("button").label(key).focusable(true).size(30.0, 30.0).radius(15.0).center()
            .fill(tint.alpha(0.2)).border(1.0, tint.alpha(0.6))
            .child(text(n.to_string()).font(type_::LABEL).color(tint))
            .on_press(move |s: &mut State| s.open_right(tab))
    };
    if pending > 0 {
        r = r.child(badge("rail-domande", pending, amber(), Tab::Questions));
    }
    if !s.session.attachments.is_empty() {
        r = r.child(badge("rail-allegati", s.session.attachments.len(), t.text_secondary, Tab::Files));
    }
    r
}

pub fn right(s: &State, k: &Kit, dark: bool) -> El {
    if !s.right_open {
        return rail(s, k, dark);
    }
    let t = tok::theme(dark);
    let counted = |name: &str, n: usize| if n == 0 { name.to_string() } else { format!("{name} {n}") };
    let labels = ["Note".to_string(), counted("File", s.session.attachments.len()), counted("Domande", s.session.questions.len())];
    let sel = match s.tab {
        Tab::Notes => 0,
        Tab::Files => 1,
        Tab::Questions => 2,
    };
    let tabs = SegmentedControl::new(&labels, Some(sel))
        .width(Some(RIGHT_W - 2.0 * space::S3 - COLLAPSE_W))
        .on_pick(|s: &mut State, i, _| s.tab = [Tab::Notes, Tab::Files, Tab::Questions][i.min(2)])
        .build(k);
    let body = match s.tab {
        Tab::Notes => notes(s, k),
        Tab::Files => files(s, k, dark),
        Tab::Questions => questions(s, k, dark),
    };
    let header = row().w_full().gap(space::S2).center_y()
        .child(tabs)
        .child(Button::new("»").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.toggle_right()).build(k).key("right-collapse"));
    col().w(RIGHT_W).h_full().min_h(0.0).padding(space::S3).gap(space::S3).fill(t.bg).border_sides([0.0, 0.0, 0.0, 1.0], t.separator)
        .child(header)
        .child(body)
}
