//! Raccolta requisiti: assistente per i colloqui con i clienti.
//! Trascrizione a sinistra, lavagna con il diagramma al centro, note e allegati a destra.

mod agent;
mod audio;
mod brand;
mod export;
mod library;
mod model;
mod settings;
mod state;
mod transcribe;
mod ui;

use rune::AppSpec;

use state::State;

fn main() {
    rune::run(AppSpec::<State> {
        min_size: Some((1000.0, 640.0)),
        scenes: &[
            ("demo", |s| *s = State::demo()),
            ("domande", |s| {
                *s = State::demo();
                s.tab = state::Tab::Questions;
            }),
            ("rec", |s| {
                *s = State::demo();
                s.start_demo_recording();
            }),
            ("audio", |s| {
                *s = State::demo();
                s.switch_area(1);
            }),
            ("export-diagram", |s| *s = State::export_diagram()),
            ("pdf-pronto", |s| {
                *s = State::demo();
                s.export = state::ExportStatus::Done("/home/utente/sessioni/rossi-2026-11-09.pdf".into());
            }),
            ("basso", |s| {
                *s = State::demo();
                s.session.add_question(Some("n6".into()), "Quanti ordini preparate in media al giorno?".into());
                s.zoom = state::Zoom::Manual(1.6);
            }),
            ("lunga", |s| {
                *s = State::demo();
                for i in 1..=30 {
                    let who = if i % 2 == 0 { model::Speaker::Cliente } else { model::Speaker::Io };
                    s.session.add_turn(who, &format!("Battuta numero {i}: il cliente racconta un passaggio del processo con abbastanza parole da andare a capo."));
                }
            }),
            ("lunga-rec", |s| {
                *s = State::demo();
                for i in 1..=30 {
                    let who = if i % 2 == 0 { model::Speaker::Cliente } else { model::Speaker::Io };
                    s.session.add_turn(who, &format!("Battuta numero {i}: il cliente racconta un passaggio del processo con abbastanza parole da andare a capo."));
                }
                s.start_demo_recording();
            }),
            ("chiuso", |s| {
                *s = State::demo();
                s.right_open = false;
            }),
            ("zoom", |s| {
                *s = State::demo();
                s.zoom = state::Zoom::Manual(1.6);
            }),
            ("nuova", |s| {
                *s = State::demo();
                s.add_area();
            }),
            ("colloqui", |s| {
                *s = State::demo();
                s.open_home();
            }),
            ("impostazioni", |s| {
                *s = State::demo();
                s.open_settings();
            }),
            ("lontano", |s| {
                *s = State::demo();
                s.zoom = state::Zoom::Manual(1.0);
                s.pan_by(-3000.0, -2000.0);
            }),
        ],
        setup: Some(|s, waker| s.setup(waker)),
        poll: Some(|s, now, tx| s.poll(now, tx)),
        on_key: Some(|s, ev, _| {
            if s.form.is_some() {
                // Il pannello delle impostazioni è sopra tutto: Esc lo chiude, il resto è suo.
                if ev.key == rune::Key::Escape {
                    s.close_settings();
                    return true;
                }
                false
            } else if s.home.is_some() {
                // Esc dal menu torna al colloquio, se ce n'è uno da riprendere.
                if ev.key == rune::Key::Escape && !s.is_blank() {
                    s.close_home();
                    return true;
                }
                false
            } else if ev.is_command('=') || ev.is_command('+') {
                s.zoom_in();
                true
            } else if ev.is_command('-') {
                s.zoom_out();
                true
            } else if ev.is_command('0') {
                s.zoom_reset();
                true
            } else if ev.key != rune::Key::Escape {
                false
            } else if s.renaming {
                s.cancel_rename();
                true
            } else if s.editing.is_some() {
                s.cancel_edit();
                true
            } else {
                false
            }
        }),
        ..AppSpec::new("Raccolta requisiti", (1440.0, 900.0), State::new, ui::view)
    });
}
