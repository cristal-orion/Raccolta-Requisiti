//! Il menu principale: i colloqui salvati, raggruppati per cliente, con la ricerca.

use rune::tok::{self, radius, space, type_};
use rune::{El, ScrollSpec, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Kit, TextInput};

use crate::library::{self, Entry};
use crate::state::{Rec, State};
use crate::ui::board::amber;

/// La larghezza massima dell'elenco: su uno schermo largo resta leggibile, al centro.
const LIST_W: f32 = 860.0;

fn card(s: &State, e: &Entry, dark: bool) -> El {
    let t = tok::theme(dark);
    let accent = tok::accent(0, dark);
    let current = s.is_current(&e.dir);
    let dir = e.dir.clone();
    let name = e.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut top = row().w_full().gap(space::S2).center_y()
        .child(text(e.areas.join(" · ")).font(type_::BODY).color(t.text).wrap().grow(1.0).min_w(0.0));
    if current {
        top = top.child(row().padding(2.0).radius(99.0).fill(accent.alpha(0.18)).child(text(" aperto ora ").font(type_::LABEL).color(accent)));
    }
    top = top.child(text(library::when_saved(e.modified)).font(type_::LABEL).color(t.text_secondary));
    let mut c = col().key(format!("sess:{name}")).role("button").label(format!("Apri il colloquio {}", e.areas.join(", "))).focusable(true)
        .w_full().padding(space::S3).gap(space::S1).radius(radius::MD)
        .fill(t.card).border(1.0, if current { accent } else { t.card_border })
        .cursor(rune::Cursor::Pointer)
        .press_scale(0.99)
        .on_press(move |s: &mut State| s.open_session(dir.clone()))
        .child(top)
        .child(text(library::stats(e)).font(type_::LABEL).color(if e.pending > 0 { amber() } else { t.text_secondary }));
    if !e.summary.is_empty() {
        c = c.child(text(e.summary.clone()).font(type_::LABEL).color(t.text_secondary).wrap());
    }
    c
}

pub fn view(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let Some(h) = &s.home else { return el() };
    let groups = library::group(&h.entries, &h.filter.text);

    let filter = TextInput::new(&h.filter)
        .width(Some(320.0))
        .placeholder("Cerca cliente, area, persona…")
        .on_edit(|s: &mut State| &mut s.home.as_mut().expect("menu aperto").filter)
        .build(k)
        .key("home-filter");
    let mut header = row().w_full().padding(space::S3).gap(space::S3).center_y().fill(t.bg).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(text("Colloqui").font(type_::TITLE).color(t.text))
        .child(filter)
        .child(el().grow(1.0));
    if !s.is_blank() {
        header = header.child(Button::new("Torna al colloquio").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.close_home()).build(k).key("home-back"));
    }
    header = header
        .child(Button::new("Impostazioni").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.open_settings()).build(k).key("settings"))
        .child(Button::new("+ Nuovo colloquio").variant(ButtonVariant::Submit).on_press(|s: &mut State| s.new_session()).build(k).key("home-new"));

    let mut list = col().w_full().max_w(LIST_W).gap(crate::ui::S4 + space::S2);
    if s.rec == Rec::Live {
        list = list.child(text(format!("Stai registrando in «{}»: aprire un altro colloquio ferma la registrazione.", s.rec_area_name().unwrap_or_default())).font(type_::LABEL).color(amber()).wrap());
    }
    if h.entries.is_empty() {
        list = list.child(text("Nessun colloquio salvato: premi «Nuovo colloquio» per cominciare.").font(type_::BODY).color(t.text_secondary).wrap());
    } else if groups.is_empty() {
        list = list.child(text(format!("Nessun colloquio corrisponde a «{}».", h.filter.text.trim())).font(type_::BODY).color(t.text_secondary).wrap());
    }
    for g in &groups {
        let count = if g.entries.len() == 1 { "1 colloquio".to_string() } else { format!("{} colloqui", g.entries.len()) };
        let mut section = col().w_full().gap(space::S2)
            .child(row().w_full().gap(space::S2).center_y()
                .child(text(g.client.clone()).font(type_::TITLE).color(t.text))
                .child(text(count).font(type_::LABEL).color(t.text_secondary)));
        for e in &g.entries {
            section = section.child(card(s, e, dark));
        }
        list = list.child(section);
    }

    col().w_full().h_full().min_h(0.0).fill(t.bg)
        .child(header)
        .child(col().key("home-list").grow(1.0).min_h(0.0).w_full().shrink().scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().padding(crate::ui::S4 + space::S2).items_center().child(list)))
}
