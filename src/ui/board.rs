//! La lavagna: un piano infinito con il diagramma Mermaid e, accanto ai nodi, le domande
//! dell'agente. Aperta = finestrella; chiusa = puntino giallo che si richiama con un clic.
//!
//! Tre strati: il «mondo» (diagramma e aree cliccabili dei nodi) segue la camera, spostamento e
//! zoom; le finestrelle ancorate ai nodi seguono solo lo spostamento e si posizionano dallo zoom,
//! così restano leggibili a ogni ingrandimento; le domande generali stanno ferme in un angolo.

use rune::tok::{self, radius, space, type_};
use rune::{Cx, El, Key, PointerButton, col, el, oklch, row, text};
use rune_kit::{Button, ButtonVariant, Kit, TextInput};
use rune_mermaid::DiagramStyle;

use crate::model::{QStatus, Question};
use crate::state::{State, node_ids, node_label};

const BUBBLE_W: f32 = 280.0;
const EDITOR_W: f32 = 300.0;
const DOT: f32 = 18.0;
/// Il margine delle cose ferme nell'angolo della lavagna.
const HUD_PAD: f32 = 12.0;

pub fn amber() -> rune::Color {
    oklch(0.84, 0.17, 92.0)
}

fn node_key(id: &str) -> String {
    format!("hover:n:{id}")
}

/// Dove sta disegnato il nodo, senza zoom, in coordinate di layout, con la sua misura.
///
/// `rune-mermaid` posiziona ogni nodo con uno spostamento grafico (`offset`) che le letture di
/// layout non contano: `hover:n:ID` risulta alla base del diagramma. Il suo figlio `lit` sta a
/// `-spostamento` dalla base, quindi lo spostamento è `base - lit` e la posizione disegnata
/// `2·base - lit`.
fn node_drawn(cx: &Cx<State>, id: &str) -> Option<([f32; 2], [f32; 2])> {
    let g = cx.layout(&node_key(id))?;
    let l = cx.layout(&format!("{}/lit", node_key(id)))?;
    Some(([2.0 * g[0] - l[0], 2.0 * g[1] - l[1]], [g[2], g[3]]))
}

/// Il nodo nel mondo, a grandezza naturale: serve all'area cliccabile, che sta nel mondo e si
/// scala con lui.
fn node_local(cx: &Cx<State>, id: &str) -> Option<[f32; 4]> {
    let ([x, y], [w, h]) = node_drawn(cx, id)?;
    let o = cx.layout("world")?;
    Some([x - o[0], y - o[1], w, h])
}

/// Il nodo come lo si vede, già ingrandito di `z`, nello strato delle finestrelle (che segue lo
/// spostamento della camera ma non lo zoom).
fn node_rect(cx: &Cx<State>, id: &str, z: f32) -> Option<[f32; 4]> {
    let [x, y, w, h] = node_local(cx, id)?;
    Some([x * z, y * z, w * z, h * z])
}

/// Dove sta una finestrella aperta rispetto al nodo `r`: sempre alla sua destra, un po' più in
/// alto, così non copre il nodo e non salta da un lato all'altro mentre ci si sposta.
fn beside(r: [f32; 4]) -> [f32; 2] {
    [r[0] + r[2] + 16.0, r[1] - 8.0]
}

/// Scende sotto le finestrelle già piazzate finché non ne tocca più nessuna.
fn avoid(placed: &[[f32; 4]], mut at: [f32; 2], size: [f32; 2]) -> [f32; 2] {
    const GAP: f32 = 8.0;
    loop {
        let hit = placed.iter().find(|p| at[0] < p[0] + p[2] && p[0] < at[0] + size[0] && at[1] < p[1] + p[3] && p[1] < at[1] + size[1]);
        match hit {
            Some(p) => at[1] = p[1] + p[3] + GAP,
            None => return at,
        }
    }
}

/// Trascinare sposta la lavagna; il disegno segue la camera senza ricostruire la vista, a meno
/// che non cambi qualcosa di visibile oltre alla camera (il pulsante «Adatta», il richiamo).
fn drag_pan(s: &mut State, d: &mut rune::Drag, tx: &mut rune::Tx) {
    let before = s.marks();
    s.drag_pan(d.phase, [d.dx, d.dy], d.cancelled());
    if s.marks() == before {
        tx.live_only();
    }
}

fn label_of(s: &State, node: &str) -> String {
    node_label(&s.session.mermaid, node).unwrap_or_else(|| node.to_string())
}

/// La finestrella per scrivere una modifica al nodo: l'agente la applica al diagramma.
fn editor(k: &Kit, s: &State, node: &str, at: [f32; 2]) -> El {
    let t = tok::theme(true);
    let accent = tok::accent(0, true);
    let header = row().w_full().gap(space::S2).center_y()
        .child(el().size(10.0, 10.0).radius(5.0).fill(accent))
        .child(text(format!("Modifica «{}»", label_of(s, node))).font(type_::LABEL).color(accent).wrap().min_w(0.0).grow(1.0));
    let input = TextInput::new(&s.node_edit)
        .width(Some(EDITOR_W - 2.0 * space::S3))
        .placeholder("Es. aggiungi un controllo dopo questo passo")
        .on_edit(|s: &mut State| &mut s.node_edit)
        .on_key(|s: &mut State, ev: &rune::KeyEvent, _tx: &mut rune::Tx| {
            if is_submit(ev) {
                s.submit_edit();
                true
            } else {
                false
            }
        })
        .build(k)
        .key("node-edit");
    let buttons = row().w_full().gap(space::S2)
        .child(el().grow(1.0))
        .child(Button::new("Annulla").variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.cancel_edit()).build(k).key("edit-cancel"))
        .child(Button::new("Applica").variant(ButtonVariant::Submit).on_press(|s: &mut State| s.submit_edit()).build(k).key("edit-apply"));
    col().key("node-editor").w(EDITOR_W).padding(space::S3).gap(space::S2).radius(radius::LG)
        .fill(t.card).border(1.0, accent.alpha(0.8)).shadow(6.0, 18.0, rune::Color::BLACK.alpha(0.35))
        .absolute(at[0], at[1])
        .child(header)
        .child(text("Scrivi cosa cambiare o aggiungere: l'agente aggiorna il diagramma.").font(type_::LABEL).color(t.text_secondary).wrap())
        .child(input)
        .child(buttons)
}

/// Un'area cliccabile sopra il nodo: un clic apre la finestrella di modifica, un trascinamento
/// sposta la lavagna come dallo sfondo.
fn hit(id: &str, r: [f32; 4]) -> El {
    let owned = id.to_string();
    el().key(format!("hit:{id}")).role("button").label(format!("Modifica il nodo {id}")).focusable(true)
        .size(r[2], r[3]).radius(radius::SM).absolute(r[0], r[1])
        .cursor(rune::Cursor::Pointer)
        .on_drag(drag_pan)
        .on_press(move |s: &mut State| {
            if !s.take_dragged() {
                s.toggle_edit(&owned);
            }
        })
}

/// Il campo di risposta di una domanda: Invio la manda all'agente.
pub fn answer_input(k: &Kit, s: &State, id: u32) -> El {
    let edit = s.answer_edits.get(&id).cloned().unwrap_or_else(|| rune::TextEdit::new(""));
    TextInput::new(&edit)
        .width(Some(BUBBLE_W - 2.0 * space::S3 - 74.0))
        .placeholder("Rispondi…")
        .on_edit(move |s: &mut State| s.answer_edit(id))
        .on_key(move |s: &mut State, ev: &rune::KeyEvent, _tx: &mut rune::Tx| {
            if is_submit(ev) {
                s.send_answer(id);
                true
            } else {
                false
            }
        })
        .build(k)
        .key(format!("answer:{id}"))
}

fn bubble(k: &Kit, s: &State, q: &Question, at: [f32; 2]) -> El {
    let t = tok::theme(true);
    let id = q.id;
    let who = q.node.as_deref().map_or("Domanda generale".to_string(), |n| format!("Dubbio su «{}»", label_of(s, n)));
    let header = row().w_full().gap(space::S2).center_y()
        .child(el().size(10.0, 10.0).radius(5.0).fill(amber()))
        .child(text(who).font(type_::LABEL).color(amber()).wrap().min_w(0.0).grow(1.0))
        .child(Button::new("Chiudi").variant(ButtonVariant::Ghost).on_press(move |s: &mut State| s.close_question(id)).build(k).key(format!("close:{id}")));
    let answer = row().w_full().gap(space::S2)
        .child(answer_input(k, s, id))
        .child(Button::new("Invia").variant(ButtonVariant::Submit).on_press(move |s: &mut State| s.send_answer(id)).build(k).key(format!("send:{id}")));
    col().key(format!("bubble:{id}")).w(BUBBLE_W).padding(space::S3).gap(space::S2).radius(radius::LG)
        .fill(oklch(0.27, 0.05, 92.0)).border(1.0, amber().alpha(0.7)).shadow(6.0, 18.0, rune::Color::BLACK.alpha(0.35))
        .absolute(at[0], at[1])
        .child(header)
        .child(text(q.text.clone()).font(type_::BODY).color(t.text).wrap())
        .child(answer)
}

fn dot(q: &Question, at: [f32; 2]) -> El {
    let id = q.id;
    el().key(format!("dot:{id}")).role("button").label("Domanda in sospeso: riapri").focusable(true)
        .size(DOT, DOT).radius(DOT / 2.0).fill(amber()).border(2.0, oklch(0.2, 0.02, 92.0))
        .shadow(2.0, 8.0, amber().alpha(0.5))
        .absolute(at[0], at[1])
        .press_scale(0.9)
        .on_press(move |s: &mut State| s.reopen_question(id))
}

/// Il cursore dello zoom: si trascina anche con il dito sul touchpad.
fn zoom_slider(s: &State, dark: bool) -> El {
    const W: f32 = 150.0;
    const KNOB: f32 = 16.0;
    let t = tok::theme(dark);
    let accent = tok::accent(0, dark);
    let x = s.zoom_frac() * (W - KNOB);
    el().key("zoom-slider").role("slider").label("Zoom").focusable(true).size(W, 24.0).cursor(rune::Cursor::Pointer)
        .child(el().absolute(0.0, 10.0).size(W, 4.0).radius(2.0).fill(t.control_track))
        .child(el().absolute(0.0, 10.0).size(x + KNOB * 0.5, 4.0).radius(2.0).fill(accent))
        .child(el().absolute(x, 4.0).size(KNOB, KNOB).radius(KNOB * 0.5).fill(t.knob).border(1.0, t.card_border))
        .on_drag(|s: &mut State, d: &mut rune::Drag, _tx: &mut rune::Tx| {
            if d.w > 0.0 {
                s.set_zoom_frac(((d.x - 8.0) / (d.w - 16.0)).clamp(0.0, 1.0));
            }
        })
}

fn zoom_bar(s: &State, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    let fit = s.zoom == crate::state::Zoom::Fit;
    let pct = (s.effective_zoom() * 100.0).round() as i32;
    let hint = match (rune::touchpad_pinch_active(), rune::touchpad_pinch_problem()) {
        (true, _) => "Due dita: sposta · Pizzico: zoom · Doppio clic: adatta".to_string(),
        (false, Some(why)) => format!("Due dita: sposta · Ctrl + rotella: zoom (pizzico non disponibile: {why})"),
        (false, None) => "Due dita o trascina: sposta · Ctrl + rotella: zoom".to_string(),
    };
    row().w_full().padding(space::S2).gap(space::S2).center_y().fill(t.bg).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(text("Zoom").font(type_::LABEL).color(t.text_secondary))
        .child(Button::new("−").variant(ButtonVariant::Outline).on_press(|s: &mut State| s.zoom_out()).build(k).key("zoom-out"))
        .child(zoom_slider(s, dark))
        .child(Button::new("+").variant(ButtonVariant::Outline).on_press(|s: &mut State| s.zoom_in()).build(k).key("zoom-in"))
        .child(Button::new(format!("{pct}%")).variant(ButtonVariant::Ghost).on_press(|s: &mut State| s.zoom_reset()).build(k).key("zoom-100"))
        .child(Button::new("Adatta").variant(if fit { ButtonVariant::Submit } else { ButtonVariant::Outline }).on_press(|s: &mut State| s.zoom_fit()).build(k).key("zoom-fit"))
        .child(el().grow(1.0))
        .child(text(hint).font(type_::LABEL).color(t.text_secondary))
}

pub fn view(s: &State, cx: &mut Cx<State>, k: &Kit, dark: bool) -> El {
    let t = tok::theme(dark);
    s.sync_camera();
    let z = s.effective_zoom();
    let mut style = DiagramStyle::neutral(dark).min_scale(0.0);
    style.max_width = 900.0;
    let diagram = rune_mermaid::render(&s.session.mermaid, &style);
    let has_diagram = diagram.is_some();
    let [vw, _] = s.viewport();

    // La scheda aderisce al diagramma: a destra resta il posto per le finestrelle.
    // (16 punti di margine per lato più il bordo.)
    let hug = cx.layout_size("diagram").map(|[w, _]| w + 2.0 * 16.0 + 1.0);
    let card = match diagram {
        Some(d) => match hug {
            Some(w) => col().w(w).child(d),
            None => d,
        },
        None => text("La lavagna si riempie man mano che il cliente racconta il processo.").font(type_::BODY).color(t.text_secondary).wrap().w(360.0),
    };

    // Il mondo si scala dall'angolo in alto a sinistra e si sposta con la camera. Ogni nodo
    // disegnato è cliccabile: l'area cliccabile sta nel mondo e si scala con lui.
    let mut world = col().key("world").absolute(0.0, 0.0).origin(0.0, 0.0)
        .live_offset(&s.live_pan).live_scale(&s.live_zoom)
        .on_layout(|s: &mut State, r: [f32; 4]| s.set_diagram_size([r[2], r[3]]))
        .child(card);
    for id in node_ids(&s.session.mermaid) {
        if let Some(r) = node_local(cx, &id) {
            world = world.child(hit(&id, r));
        }
    }

    // Lo strato delle finestrelle ancorate: segue lo spostamento, non lo zoom.
    let mut pinned = el().key("pinned").absolute(0.0, 0.0).live_offset(&s.live_pan);
    // Le cose ferme nell'angolo: domande generali e il richiamo al diagramma.
    let mut corner: Vec<El> = Vec::new();

    if let Some(r) = s.editing.as_deref().and_then(|n| node_rect(cx, n, z)) {
        pinned = pinned.child(el().key("edit-ring").absolute(r[0] - 4.0, r[1] - 4.0).size(r[2] + 8.0, r[3] + 8.0).radius(radius::MD)
            .border(2.0, tok::accent(0, dark)).inert());
    }

    // Le finestrelle aperte non si sovrappongono tra loro e non coprono i puntini: i puntini si
    // piazzano per primi come ostacoli, poi ogni finestrella scende sotto quello che trova.
    // Quelle ancorate e quelle nell'angolo stanno in strati diversi: ognuno ha i suoi ostacoli.
    let mut placed: Vec<[f32; 4]> = Vec::new();
    let mut placed_corner: Vec<[f32; 4]> = Vec::new();
    let mut dots: Vec<El> = Vec::new();
    let mut dots_at: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut loose_dots = 0u32;
    let height_of = |cx: &Cx<State>, key: &str, fallback: f32| cx.layout_size(key).map_or(fallback, |sz| sz[1]);
    for q in s.session.questions.iter().filter(|q| q.status == QStatus::Parked) {
        match q.node.as_deref().and_then(|n| node_rect(cx, n, z).map(|r| (n, r))) {
            Some((n, r)) => {
                let i = dots_at.entry(n).or_insert(0);
                let at = [r[0] + r[2] - DOT * 0.5 + *i as f32 * (DOT + 4.0), r[1] - DOT * 0.5];
                *i += 1;
                placed.push([at[0], at[1], DOT, DOT]);
                dots.push(dot(q, at));
            }
            None => {
                let at = [vw - DOT - HUD_PAD, HUD_PAD + loose_dots as f32 * (DOT + 4.0)];
                loose_dots += 1;
                placed_corner.push([at[0], at[1], DOT, DOT]);
                corner.push(dot(q, at));
            }
        }
    }
    for q in s.session.questions.iter().filter(|q| q.status == QStatus::Open) {
        let h = height_of(cx, &format!("bubble:{}", q.id), 200.0);
        match q.node.as_deref().and_then(|n| node_rect(cx, n, z)) {
            Some(r) => {
                let at = avoid(&placed, beside(r), [BUBBLE_W, h]);
                placed.push([at[0], at[1], BUBBLE_W, h]);
                pinned = pinned.child(bubble(k, s, q, at));
            }
            None => {
                let at = avoid(&placed_corner, [vw - BUBBLE_W - HUD_PAD, HUD_PAD], [BUBBLE_W, h]);
                placed_corner.push([at[0], at[1], BUBBLE_W, h]);
                corner.push(bubble(k, s, q, at));
            }
        }
    }
    if let Some(n) = s.editing.as_deref() {
        let h = height_of(cx, "node-editor", 170.0);
        match node_rect(cx, n, z) {
            Some(r) => pinned = pinned.child(editor(k, s, n, avoid(&placed, beside(r), [EDITOR_W, h]))),
            None => corner.push(editor(k, s, n, avoid(&placed_corner, [vw - EDITOR_W - HUD_PAD, HUD_PAD], [EDITOR_W, h]))),
        }
    }
    for d in dots {
        pinned = pinned.child(d);
    }
    if has_diagram && !s.diagram_on_screen() {
        corner.push(
            row().key("lost").absolute(HUD_PAD, HUD_PAD).padding(space::S2).gap(space::S2).center_y().radius(radius::LG)
                .fill(t.card).border(1.0, t.card_border).shadow(4.0, 14.0, rune::Color::BLACK.alpha(0.3))
                .child(text("Il diagramma è fuori vista").font(type_::LABEL).color(t.text_secondary))
                .child(Button::new("Torna al diagramma").variant(ButtonVariant::Submit).on_press(|s: &mut State| s.zoom_fit()).build(k).key("back-to-diagram")),
        );
    }

    let summary = col().w_full().padding(crate::ui::S4).gap(space::S1).fill(t.card).border(1.0, t.card_border)
        .child(text("Riassunto").font(type_::LABEL).color(t.text_secondary))
        .child(text(if s.session.summary.is_empty() { "Ancora niente: appena c'è materiale l'agente lo riassume qui.".to_string() } else { s.session.summary.clone() })
            .font(type_::BODY).color(t.text).wrap());

    // La lavagna: due dita la spostano (con inerzia), il pizzico e Ctrl + rotella ingrandiscono
    // attorno al puntatore, lo sfondo e il tasto centrale si trascinano, il doppio clic adatta.
    let viewport = col().key("board-view").grow(1.0).min_h(0.0).w_full().clip_soft(0.0)
        .cursor(rune::Cursor::Grab)
        .on_layout(|s: &mut State, r: [f32; 4]| s.set_viewport_size([r[2], r[3]]))
        .on_wheel(|s: &mut State, e: rune::WheelEvent, tx: &mut rune::Tx| {
            let before = s.marks();
            s.wheel(e, tx);
            if s.marks() == before {
                tx.live_only();
            }
            true
        })
        .on_magnify(|s: &mut State, e: rune::MagnifyEvent, _tx: &mut rune::Tx| {
            s.zoom_at(1.0 + e.delta, [e.x, e.y]);
            true
        })
        .pointer_buttons([PointerButton::Middle])
        .on_drag(drag_pan)
        .on_double_press(|s: &mut State| s.zoom_toggle())
        .child(world)
        .child(pinned)
        .children(corner);

    col().grow(1.0).min_h(0.0).min_w(0.0).fill(t.bg)
        .child(zoom_bar(s, k, dark))
        .child(viewport)
        .child(summary)
}

// Il tasto Invio nelle risposte: lo gestisce il campo, vedi `answer_input`.
pub fn is_submit(ev: &rune::KeyEvent) -> bool {
    ev.key == Key::Enter && !ev.mods.shift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_goes_right_of_its_node_slightly_higher() {
        let node = [100.0, 200.0, 150.0, 50.0];
        assert_eq!(beside(node), [266.0, 192.0]);
        let [x, _] = beside([600.0, 0.0, 150.0, 50.0]);
        assert!(x >= 750.0, "mai sopra il nodo, anche al bordo: la lavagna non ha bordi");
    }

    #[test]
    fn a_question_on_a_node_at_the_top_is_not_pushed_down() {
        assert_eq!(beside([10.0, 0.0, 50.0, 20.0])[1], -8.0, "sul piano infinito non serve stare dentro un bordo");
    }

    #[test]
    fn questions_stack_below_what_is_already_there() {
        let placed = [[100.0, 100.0, 280.0, 200.0]];
        assert_eq!(avoid(&placed, [120.0, 150.0], [280.0, 100.0]), [120.0, 308.0]);
        assert_eq!(avoid(&placed, [500.0, 150.0], [280.0, 100.0]), [500.0, 150.0], "a lato non tocca");
    }
}
