//! La camera della lavagna. La lavagna è infinita: il diagramma e le finestrelle stanno in un
//! «mondo» che si sposta con due dita sul touchpad (con un po' di inerzia quando le dita si
//! staccano), trascinando lo sfondo o con il tasto centrale, e si ingrandisce con il pizzico
//! attorno alle dita, Ctrl + rotella o i pulsanti. Il disegno segue la camera tramite due valori
//! dal vivo (`live_pan`, `live_zoom`): spostarsi non ricostruisce la vista.

use std::time::Duration;

use rune::{GesturePhase, Phase, Tx, WheelEvent};

use super::State;

/// Come si guarda il diagramma: adattato alla lavagna (si rimpicciolisce se cresce) oppure a
/// un ingrandimento scelto, con la lavagna spostata dove l'ha lasciata la mano.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    Fit,
    Manual(f32),
}

pub const ZOOM_MIN: f32 = 0.25;
pub const ZOOM_MAX: f32 = 3.0;
/// Dove sta l'angolo del diagramma quando è adattato: un po' di respiro a sinistra e in alto.
pub const CANVAS_MARGIN: f32 = 56.0;
/// Quanto resta libero dal lato opposto quando si adatta il diagramma alla lavagna.
const CANVAS_FAR_PAD: f32 = 24.0;
/// Quanto dura l'inerzia: la velocità si riduce a un terzo circa in questo tempo (secondi).
const GLIDE_TAU: f32 = 0.3;
/// Sotto questa velocità (punti al secondo) l'inerzia si ferma.
const GLIDE_STOP: f32 = 25.0;
/// Una spinta più lenta di così non lancia niente: è la mano che si ferma.
const GLIDE_START: f32 = 120.0;
const GLIDE_MAX: f32 = 5000.0;
/// Gli ultimi movimenti delle dita da cui si misura la velocità (secondi).
const SAMPLE_WINDOW: f64 = 0.1;
/// Un trascinamento più corto di così (punti) è un clic, non uno spostamento.
const DRAG_SLOP: f32 = 4.0;

/// La lavagna che scivola da sola dopo che le dita si sono staccate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Glide {
    v: [f32; 2],
    last: f64,
}

/// I movimenti recenti delle dita: (istante in secondi, spostamento).
pub(super) type Samples = Vec<(f64, [f32; 2])>;

/// La velocità con cui la mano lascia la lavagna, dai movimenti degli ultimi istanti; niente se
/// le dita si erano già fermate o andavano piano.
pub(super) fn release_velocity(samples: &[(f64, [f32; 2])], now: f64) -> Option<[f32; 2]> {
    let recent: Vec<&(f64, [f32; 2])> = samples.iter().filter(|s| now - s.0 <= SAMPLE_WINDOW).collect();
    let (first, last) = (recent.first()?, recent.last()?);
    // Ferme da più di un paio di fotogrammi prima di staccare: nessun lancio.
    if recent.len() < 2 || now - last.0 > 0.05 {
        return None;
    }
    // Più movimenti nello stesso fotogramma hanno lo stesso istante: almeno un fotogramma.
    let span = (last.0 - first.0).max(1.0 / 60.0) as f32;
    // Il primo campione segna l'inizio dell'intervallo: si contano gli spostamenti dopo.
    let sum = recent[1..].iter().fold([0.0f32; 2], |a, s| [a[0] + s.1[0], a[1] + s.1[1]]);
    let v = [sum[0] / span, sum[1] / span];
    let speed = v[0].hypot(v[1]);
    if speed < GLIDE_START {
        return None;
    }
    let k = (GLIDE_MAX / speed).min(1.0);
    Some([v[0] * k, v[1] * k])
}

/// Un passo dell'inerzia lungo `dt` secondi: lo spostamento e la velocità che resta.
pub(super) fn glide_step(v: [f32; 2], dt: f32) -> ([f32; 2], [f32; 2]) {
    let k = (-dt / GLIDE_TAU).exp();
    let travel = GLIDE_TAU * (1.0 - k);
    ([v[0] * travel, v[1] * travel], [v[0] * k, v[1] * k])
}

/// Quello che, cambiando, chiede di ridisegnare la vista e non solo di spostare la camera: il
/// pulsante «Adatta» acceso o spento e il richiamo «Torna al diagramma».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marks {
    fit: bool,
    on_screen: bool,
}

impl State {
    pub fn set_diagram_size(&mut self, size: [f32; 2]) {
        self.diagram_size = size;
        self.sync_camera();
    }

    pub fn set_viewport_size(&mut self, size: [f32; 2]) {
        self.viewport_size = size;
        self.sync_camera();
    }

    /// Lo spazio visibile della lavagna (zero finché non è stato misurato).
    pub fn viewport(&self) -> [f32; 2] {
        self.viewport_size
    }

    /// L'ingrandimento che si vede adesso. Adattato: quanto serve perché il diagramma stia
    /// tutto nella lavagna, senza mai ingrandirlo oltre il 100%.
    pub fn effective_zoom(&self) -> f32 {
        match self.zoom {
            Zoom::Manual(z) => z,
            Zoom::Fit => {
                let (d, v) = (self.diagram_size, self.viewport_size);
                if d[0] < 1.0 || d[1] < 1.0 || v[0] < 1.0 || v[1] < 1.0 {
                    return 1.0;
                }
                ((v[0] - CANVAS_MARGIN - CANVAS_FAR_PAD) / d[0]).min((v[1] - CANVAS_MARGIN - CANVAS_FAR_PAD) / d[1]).clamp(ZOOM_MIN, 1.0)
            }
        }
    }

    /// Dove sta l'angolo in alto a sinistra del diagramma nella lavagna.
    pub fn pan(&self) -> [f32; 2] {
        match self.zoom {
            Zoom::Fit => [CANVAS_MARGIN, CANVAS_MARGIN],
            Zoom::Manual(_) => self.pan,
        }
    }

    /// Porta la camera ai valori dal vivo che muovono il disegno. Non cambia niente se sono già
    /// quelli: la vista la chiama a ogni giro.
    pub fn sync_camera(&self) {
        self.live_pan.set(self.pan());
        self.live_zoom.set(self.effective_zoom());
    }

    fn set_camera(&mut self, pan: [f32; 2], zoom: f32) {
        let ok = |v: f32| if v.is_finite() { v } else { 0.0 };
        self.pan = [ok(pan[0]), ok(pan[1])];
        self.zoom = Zoom::Manual(zoom.clamp(ZOOM_MIN, ZOOM_MAX));
        self.sync_camera();
    }

    pub fn marks(&self) -> Marks {
        Marks { fit: self.zoom == Zoom::Fit, on_screen: self.diagram_on_screen() }
    }

    /// Il diagramma si vede almeno in parte (vero anche finché le misure non ci sono).
    pub fn diagram_on_screen(&self) -> bool {
        let (p, z, d, v) = (self.pan(), self.effective_zoom(), self.diagram_size, self.viewport_size);
        if d[0] < 1.0 || v[0] < 1.0 {
            return true;
        }
        p[0] < v[0] && p[0] + d[0] * z > 0.0 && p[1] < v[1] && p[1] + d[1] * z > 0.0
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        let (p, z) = (self.pan(), self.effective_zoom());
        self.set_camera([p[0] + dx, p[1] + dy], z);
    }

    /// Porta lo zoom a `target` tenendo fermo il punto della lavagna sotto `at`.
    pub fn zoom_to(&mut self, target: f32, at: [f32; 2]) {
        self.glide = None;
        let (p, z) = (self.pan(), self.effective_zoom());
        let nz = target.clamp(ZOOM_MIN, ZOOM_MAX);
        let world = [(at[0] - p[0]) / z, (at[1] - p[1]) / z];
        self.set_camera([at[0] - world[0] * nz, at[1] - world[1] * nz], nz);
    }

    /// Il pizzico e Ctrl + rotella: ingrandisce attorno al punto `at` della lavagna.
    pub fn zoom_at(&mut self, factor: f32, at: [f32; 2]) {
        self.zoom_to(self.effective_zoom() * factor, at);
    }

    fn centre(&self) -> [f32; 2] {
        [self.viewport_size[0] * 0.5, self.viewport_size[1] * 0.5]
    }

    /// I pulsanti e la tastiera: ingrandiscono attorno al centro della lavagna.
    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom_at(factor, self.centre());
    }

    pub fn zoom_in(&mut self) {
        self.zoom_by(1.25);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_by(0.8);
    }

    /// Dove sta lo zoom sul cursore (0 = minimo, 1 = massimo), su scala logaritmica: ogni passo
    /// del cursore moltiplica l'ingrandimento per lo stesso fattore.
    pub fn zoom_frac(&self) -> f32 {
        ((self.effective_zoom() / ZOOM_MIN).ln() / (ZOOM_MAX / ZOOM_MIN).ln()).clamp(0.0, 1.0)
    }

    pub fn set_zoom_frac(&mut self, frac: f32) {
        self.zoom_to(ZOOM_MIN * (ZOOM_MAX / ZOOM_MIN).powf(frac.clamp(0.0, 1.0)), self.centre());
    }

    /// Doppio clic sulla lavagna: da «Adatta» al 100%, e da qualsiasi altro livello a «Adatta».
    pub fn zoom_toggle(&mut self) {
        if self.zoom == Zoom::Fit {
            self.zoom_to(1.0, self.centre());
        } else {
            self.zoom_fit();
        }
    }

    pub fn zoom_reset(&mut self) {
        self.zoom_to(1.0, self.centre());
    }

    pub fn zoom_fit(&mut self) {
        self.glide = None;
        self.zoom = Zoom::Fit;
        self.sync_camera();
    }

    // ---- la mano sulla lavagna ----

    /// Due dita sul touchpad, o la rotella: sposta la lavagna. Quando le dita si staccano mentre
    /// la lavagna corre, la lascia scivolare.
    pub fn wheel(&mut self, e: WheelEvent, tx: &mut Tx) {
        // L'orologio dei fotogrammi: lo stesso dell'inerzia, e virtuale nelle prove senza finestra.
        let t = tx.clock();
        if e.phase == GesturePhase::Began {
            self.glide = None;
            self.samples.clear();
        }
        if e.dx != 0.0 || e.dy != 0.0 {
            self.glide = None;
            self.pan_by(e.dx, e.dy);
            if e.precise {
                self.samples.push((t, [e.dx, e.dy]));
                self.samples.retain(|s| t - s.0 <= SAMPLE_WINDOW);
            }
        }
        if e.phase == GesturePhase::Ended {
            if let Some(v) = release_velocity(&self.samples, t) {
                self.glide = Some(Glide { v, last: t });
                tx.wake_after(Duration::ZERO);
            }
            self.samples.clear();
        }
    }

    /// Un passo dell'inerzia, da `poll`. Torna true se serve ridisegnare la vista.
    pub(super) fn step_glide(&mut self, now: f64, tx: &mut Tx) -> bool {
        let Some(g) = self.glide else { return false };
        let dt = (now - g.last).clamp(0.0, 0.05) as f32;
        if dt <= 0.0 {
            tx.wake_after(Duration::from_millis(8));
            return false;
        }
        let before = self.marks();
        let (step, v) = glide_step(g.v, dt);
        self.pan_by(step[0], step[1]);
        self.glide = (v[0].hypot(v[1]) >= GLIDE_STOP).then_some(Glide { v, last: now });
        if self.glide.is_some() {
            tx.wake_after(Duration::from_millis(8));
        }
        self.marks() != before
    }

    /// Trascinare lo sfondo (o un nodo, o con il tasto centrale) sposta la lavagna: `delta` è il
    /// cammino del puntatore dalla pressione, in punti della finestra.
    pub fn drag_pan(&mut self, phase: Phase, delta: [f32; 2], cancelled: bool) {
        if phase == Phase::Down {
            self.glide = None;
            self.pan_from = self.pan();
            self.dragged = false;
            return;
        }
        if cancelled {
            if self.dragged {
                let z = self.effective_zoom();
                self.set_camera(self.pan_from, z);
            }
            return;
        }
        self.dragged |= delta[0].abs() > DRAG_SLOP || delta[1].abs() > DRAG_SLOP;
        if self.dragged {
            let z = self.effective_zoom();
            self.set_camera([self.pan_from[0] + delta[0], self.pan_from[1] + delta[1]], z);
        }
    }

    /// Dopo un trascinamento il rilascio non vale come clic sul nodo: lo dice e lo dimentica.
    pub fn take_dragged(&mut self) -> bool {
        std::mem::take(&mut self.dragged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> State {
        State::with_dir(std::env::temp_dir().join("rr-camera-test"), false)
    }

    fn sized(diagram: [f32; 2], viewport: [f32; 2]) -> State {
        let mut s = quiet();
        s.set_diagram_size(diagram);
        s.set_viewport_size(viewport);
        s
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn fit_shrinks_a_tall_diagram_to_the_board_but_never_enlarges_a_small_one() {
        let tall = sized([800.0, 1000.0], [600.0, 500.0]);
        assert!((tall.effective_zoom() - (500.0 - CANVAS_MARGIN - CANVAS_FAR_PAD) / 1000.0).abs() < 1e-4, "{}", tall.effective_zoom());
        let small = sized([200.0, 300.0], [1000.0, 800.0]);
        assert_eq!(small.effective_zoom(), 1.0);
        let huge = sized([20000.0, 20000.0], [600.0, 500.0]);
        assert_eq!(huge.effective_zoom(), ZOOM_MIN, "oltre il limite resta leggibile da spostare");
        assert_eq!(small.pan(), [CANVAS_MARGIN, CANVAS_MARGIN]);
    }

    #[test]
    fn until_the_sizes_are_known_the_zoom_is_one() {
        assert_eq!(quiet().effective_zoom(), 1.0);
        assert_eq!(sized([0.0, 0.0], [600.0, 500.0]).effective_zoom(), 1.0);
    }

    #[test]
    fn zooming_leaves_fit_for_a_manual_level_starting_from_what_was_on_screen() {
        let mut s = sized([800.0, 1000.0], [600.0, 500.0]);
        let seen = s.effective_zoom();
        s.zoom_in();
        assert_eq!(s.zoom, Zoom::Manual(seen * 1.25));
        s.zoom_reset();
        assert_eq!(s.effective_zoom(), 1.0);
        s.zoom_out();
        assert!(near(s.effective_zoom(), 0.8));
        s.zoom_fit();
        assert_eq!(s.zoom, Zoom::Fit);
    }

    #[test]
    fn the_zoom_stays_between_its_limits() {
        let mut s = quiet();
        for _ in 0..40 {
            s.zoom_in();
        }
        assert_eq!(s.effective_zoom(), ZOOM_MAX);
        for _ in 0..80 {
            s.zoom_out();
        }
        assert_eq!(s.effective_zoom(), ZOOM_MIN);
        s.zoom_by(1.0 + 0.2);
        assert!((s.effective_zoom() - ZOOM_MIN * 1.2).abs() < 1e-4, "la pinza e Ctrl+rotella usano lo stesso passaggio");
    }

    #[test]
    fn a_pinch_keeps_the_point_under_the_fingers_still() {
        let mut s = sized([400.0, 600.0], [800.0, 600.0]);
        s.zoom = Zoom::Manual(1.0);
        s.pan = [100.0, 50.0];
        let at = [300.0, 250.0];
        let world = |s: &State| {
            let (p, z) = (s.pan(), s.effective_zoom());
            [(at[0] - p[0]) / z, (at[1] - p[1]) / z]
        };
        let before = world(&s);
        s.zoom_at(1.6, at);
        assert!(near(s.effective_zoom(), 1.6));
        let after = world(&s);
        assert!(near(before[0], after[0]) && near(before[1], after[1]), "{before:?} → {after:?}");
    }

    #[test]
    fn panning_has_no_edge_in_any_direction() {
        let mut s = sized([400.0, 600.0], [800.0, 600.0]);
        s.zoom = Zoom::Manual(1.6);
        s.pan = [CANVAS_MARGIN, CANVAS_MARGIN];
        // Verso l'alto e verso sinistra oltre l'inizio del diagramma: nessun blocco.
        s.pan_by(0.0, 5000.0);
        s.pan_by(5000.0, 0.0);
        assert_eq!(s.pan(), [CANVAS_MARGIN + 5000.0, CANVAS_MARGIN + 5000.0]);
        s.pan_by(-20000.0, -20000.0);
        assert_eq!(s.pan(), [CANVAS_MARGIN - 15000.0, CANVAS_MARGIN - 15000.0]);
        assert!(!s.diagram_on_screen(), "così lontano il diagramma non si vede più");
        s.zoom_fit();
        assert!(s.diagram_on_screen(), "«Adatta» lo riporta in vista");
    }

    #[test]
    fn panning_from_fit_keeps_the_zoom_that_was_on_screen() {
        let mut s = sized([800.0, 1000.0], [600.0, 500.0]);
        let seen = s.effective_zoom();
        s.pan_by(10.0, -30.0);
        assert_eq!(s.zoom, Zoom::Manual(seen));
        assert_eq!(s.pan(), [CANVAS_MARGIN + 10.0, CANVAS_MARGIN - 30.0]);
    }

    #[test]
    fn the_live_camera_follows_every_change() {
        let mut s = sized([800.0, 1000.0], [600.0, 500.0]);
        assert_eq!(s.live_zoom.get(), s.effective_zoom());
        s.pan_by(12.0, 7.0);
        assert_eq!(s.live_pan.get(), s.pan());
        s.zoom_at(2.0, [100.0, 100.0]);
        assert_eq!(s.live_zoom.get(), s.effective_zoom());
        assert_eq!(s.live_pan.get(), s.pan());
    }

    #[test]
    fn the_zoom_slider_is_logarithmic_and_its_ends_are_the_limits() {
        let mut s = quiet();
        s.set_zoom_frac(0.0);
        assert!((s.effective_zoom() - ZOOM_MIN).abs() < 1e-5);
        s.set_zoom_frac(1.0);
        assert!((s.effective_zoom() - ZOOM_MAX).abs() < 1e-4);
        s.set_zoom_frac(0.5);
        assert!((s.effective_zoom() - (ZOOM_MIN * ZOOM_MAX).sqrt()).abs() < 1e-4, "a metà sta la media geometrica");
        for z in [0.25_f32, 0.6, 1.0, 2.2, 3.0] {
            s.zoom = Zoom::Manual(z);
            let f = s.zoom_frac();
            s.set_zoom_frac(f);
            assert!((s.effective_zoom() - z).abs() < 1e-3, "{z} → {f} → {}", s.effective_zoom());
        }
        s.set_zoom_frac(7.0);
        assert_eq!(s.effective_zoom(), ZOOM_MAX, "fuori scala si ferma al limite");
    }

    #[test]
    fn a_double_click_goes_from_fit_to_100_percent_and_from_anything_else_back_to_fit() {
        let mut s = quiet();
        s.zoom_toggle();
        assert_eq!(s.zoom, Zoom::Manual(1.0));
        s.zoom_toggle();
        assert_eq!(s.zoom, Zoom::Fit);
        s.zoom = Zoom::Manual(2.0);
        s.zoom_toggle();
        assert_eq!(s.zoom, Zoom::Fit);
    }

    #[test]
    fn a_quick_flick_glides_and_a_hand_that_stopped_does_not() {
        // 10 movimenti da 20 punti in 80 ms: 2500 punti al secondo verso il basso.
        let flick: Vec<(f64, [f32; 2])> = (0..10).map(|i| (1.0 + i as f64 * 0.008 + 0.008, [0.0, 20.0])).collect();
        let v = release_velocity(&flick, 1.09).expect("lancia");
        assert!(v[0].abs() < 1e-3 && (v[1] - 2500.0).abs() < 1.0, "{v:?}");
        assert_eq!(release_velocity(&flick, 1.3), None, "dita ferme prima di staccare");
        let slow: Vec<(f64, [f32; 2])> = (0..10).map(|i| (1.0 + i as f64 * 0.01, [0.0, 0.5])).collect();
        assert_eq!(release_velocity(&slow, 1.1), None, "troppo piano per scivolare");
        assert_eq!(release_velocity(&[], 1.0), None);
    }

    #[test]
    fn a_glide_slows_down_and_covers_a_bounded_distance() {
        let mut v = [0.0, 2000.0];
        let mut travelled = 0.0;
        for _ in 0..600 {
            let (step, next) = glide_step(v, 1.0 / 120.0);
            travelled += step[1];
            v = next;
        }
        assert!(v[1] < GLIDE_STOP, "si ferma da sola: {v:?}");
        assert!((travelled - 2000.0 * GLIDE_TAU).abs() < 5.0, "percorre velocità × tau: {travelled}");
    }

    #[test]
    fn a_short_drag_is_a_click_and_a_long_one_pans() {
        let mut s = sized([400.0, 600.0], [800.0, 600.0]);
        s.drag_pan(Phase::Down, [0.0, 0.0], false);
        s.drag_pan(Phase::Up, [2.0, 0.0], false);
        assert_eq!(s.zoom, Zoom::Fit, "un clic non sposta niente");
        assert!(!s.take_dragged());
        s.drag_pan(Phase::Down, [0.0, 0.0], false);
        s.drag_pan(Phase::Move, [40.0, -25.0], false);
        assert_eq!(s.pan(), [CANVAS_MARGIN + 40.0, CANVAS_MARGIN - 25.0]);
        s.drag_pan(Phase::Up, [40.0, -25.0], false);
        assert!(s.take_dragged(), "il rilascio dopo uno spostamento non apre il nodo");
        assert!(!s.take_dragged());
        s.drag_pan(Phase::Down, [0.0, 0.0], false);
        s.drag_pan(Phase::Move, [90.0, 0.0], false);
        s.drag_pan(Phase::Up, [90.0, 0.0], true);
        assert_eq!(s.pan(), [CANVAS_MARGIN + 40.0, CANVAS_MARGIN - 25.0], "annullato: torna dov'era");
    }
}
