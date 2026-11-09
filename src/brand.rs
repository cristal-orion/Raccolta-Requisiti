//! Il brand TwoBee per i documenti: colori e font dal manuale, i font incorporati nel PDF e il
//! CSS che li applica.
//!
//! Il manuale si legge da `RR_BRAND_FILE`, altrimenti da `~/Projects/Twobee/Twobee/brand-colori-font.txt`
//! (il file del sito), altrimenti dalla copia salvata in `assets/brand/`. Così un manuale
//! aggiornato vale subito, senza ricompilare, e l'app funziona anche se il file non c'è.

use std::path::PathBuf;

use base64::Engine;

const EMBEDDED_MANUAL: &str = include_str!("../assets/brand/twobee-brand.txt");
const SITE_MANUAL: &str = "Projects/Twobee/Twobee/brand-colori-font.txt";

static SPARTAN: &[u8] = include_bytes!("../assets/fonts/LeagueSpartan-Variable.ttf");
static INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
static INTER_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

#[derive(Clone, Debug)]
pub struct Brand {
    /// Il testo del manuale, com'è: lo legge anche il modello.
    pub manual: String,
    pub yellow: String,
    pub yellow_light: String,
    pub yellow_dark: String,
    pub black: String,
    pub dark: String,
    pub offwhite: String,
    pub white: String,
    pub title_font: String,
    pub body_font: String,
}

/// `#RRGGBB` nella prima riga che comincia con `label` (senza badare alle maiuscole).
fn hex_on_line(manual: &str, label: &str) -> Option<String> {
    manual.lines().map(str::trim_start).find(|l| l.to_lowercase().starts_with(&label.to_lowercase())).and_then(|l| {
        let at = l.find('#')?;
        let hex = l.get(at..at + 7)?;
        (hex[1..].chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_uppercase())
    })
}

/// Il nome del font nella prima riga non vuota dopo l'intestazione `header` (`League Spartan   — weight 800…`).
fn font_after(manual: &str, header: &str) -> Option<String> {
    let mut lines = manual.lines();
    lines.find(|l| l.trim_start().to_lowercase().starts_with(&header.to_lowercase()))?;
    let line = lines.find(|l| !l.trim().is_empty())?;
    let name = line.trim().split('—').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn manual_text() -> String {
    let from_env = std::env::var_os("RR_BRAND_FILE").map(PathBuf::from);
    let site = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(SITE_MANUAL));
    [from_env, site].into_iter().flatten().find_map(|p| std::fs::read_to_string(p).ok()).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| EMBEDDED_MANUAL.to_string())
}

impl Default for Brand {
    fn default() -> Self {
        Brand::parse(EMBEDDED_MANUAL)
    }
}

impl Brand {
    pub fn load() -> Brand {
        Brand::parse(&manual_text())
    }

    /// I valori che il manuale non dà (o dà storti) restano quelli del brand.
    pub fn parse(manual: &str) -> Brand {
        let pick = |label: &str, fallback: &str| hex_on_line(manual, label).unwrap_or_else(|| fallback.to_string());
        Brand {
            manual: manual.trim().to_string(),
            yellow: pick("Default", "#FFC501"),
            yellow_light: pick("Light .", "#FFE27A"),
            yellow_dark: pick("Dark / hover", "#E0A800"),
            black: pick("Nero brand", "#0B0B0C"),
            dark: pick("Dark .", "#111113"),
            offwhite: pick("Off-white", "#FBFAF9"),
            white: pick("Bianco puro", "#FFFFFF"),
            title_font: font_after(manual, "TITOLI").unwrap_or_else(|| "League Spartan".into()),
            body_font: font_after(manual, "TESTO").unwrap_or_else(|| "Inter".into()),
        }
    }

    /// L'elenco dei font per il CSS: quello del manuale, poi quelli di ripiego.
    fn stack(&self, first: &str, fallback: &[&str]) -> String {
        let mut names = vec![format!("'{first}'")];
        names.extend(fallback.iter().filter(|f| !first.eq_ignore_ascii_case(f.trim_matches('\''))).map(|f| f.to_string()));
        names.join(", ")
    }

    /// Variabili, font incorporati e regole del brand: da mettere nel `<head>`, dopo il CSS del modello.
    pub fn css(&self, with_cover: bool) -> String {
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let face = |family: &str, bytes: &[u8], weight: &str| format!("@font-face{{font-family:'{family}';src:url(data:font/ttf;base64,{}) format('truetype');font-weight:{weight};font-style:normal;}}", b64(bytes));
        let fonts = [
            face("League Spartan", SPARTAN, "100 900"),
            face("Inter", INTER_REGULAR, "300 400"),
            face("Inter", INTER_MEDIUM, "500"),
            face("Inter", INTER_SEMIBOLD, "600 900"),
        ]
        .join("\n");
        let title = self.stack(&self.title_font, &["'Inter'", "system-ui", "sans-serif"]);
        let body = self.stack(&self.body_font, &["system-ui", "sans-serif"]);
        let cover_page = if with_cover { "@page :first { margin: 0; }\n" } else { "" };
        format!(
            r#"{fonts}
:root {{
  --tb-yellow: {y}; --tb-yellow-light: {yl}; --tb-yellow-dark: {yd};
  --tb-black: {k}; --tb-dark: {d}; --tb-offwhite: {o}; --tb-white: {w};
  --tb-font-title: {title}; --tb-font-body: {body};
}}
* {{ -webkit-print-color-adjust: exact; print-color-adjust: exact; }}
{cover_page}html, body {{ font-family: var(--tb-font-body) !important; color: var(--tb-black); background: var(--tb-white); }}
h1, h2, h3, h4, .tb-title {{ font-family: var(--tb-font-title) !important; font-weight: 800 !important; color: var(--tb-black); }}
h2::before, .tb-hex {{ content: ""; display: inline-block; width: .5em; height: .56em; margin-right: .45em; background: var(--tb-yellow); clip-path: polygon(50% 0, 100% 25%, 100% 75%, 50% 100%, 0 75%, 0 25%); }}
.tb-hex {{ vertical-align: baseline; }}
table {{ border-collapse: collapse; }}
th {{ background: var(--tb-black) !important; color: var(--tb-yellow) !important; font-family: var(--tb-font-title); font-weight: 800; }}
td {{ border-bottom: 1px solid #e4e1db; }}
.tb-cover {{ background: var(--tb-black); color: var(--tb-offwhite); box-sizing: border-box; height: 296mm; padding: 32mm 22mm; break-after: page; }}
.tb-cover h1 {{ color: var(--tb-offwhite) !important; font-size: 34pt; line-height: 1.1; margin: 0 0 8mm; padding-bottom: 8mm; border-bottom: 6px solid var(--tb-yellow); }}
.tb-cover h2, .tb-cover h3, .tb-cover p, .tb-cover li, .tb-cover td {{ color: var(--tb-offwhite) !important; }}
.tb-cover h2::before {{ background: var(--tb-yellow); }}
.tb-cover th {{ background: transparent !important; color: var(--tb-yellow) !important; border-bottom: 2px solid var(--tb-yellow); }}
.tb-cover td {{ border-bottom: 1px solid #3a3a3d; }}
"#,
            y = self.yellow,
            yl = self.yellow_light,
            yd = self.yellow_dark,
            k = self.black,
            d = self.dark,
            o = self.offwhite,
            w = self.white,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_manual_gives_the_brand_colors_and_fonts() {
        let b = Brand::default();
        assert_eq!((b.yellow.as_str(), b.yellow_light.as_str(), b.yellow_dark.as_str()), ("#FFC501", "#FFE27A", "#E0A800"));
        assert_eq!((b.black.as_str(), b.dark.as_str(), b.offwhite.as_str(), b.white.as_str()), ("#0B0B0C", "#111113", "#FBFAF9", "#FFFFFF"));
        assert_eq!((b.title_font.as_str(), b.body_font.as_str()), ("League Spartan", "Inter"));
        assert!(b.manual.contains("GIALLO BRAND"), "il modello riceve il manuale intero");
    }

    #[test]
    fn a_changed_manual_changes_the_brand_and_a_broken_one_keeps_the_defaults() {
        let b = Brand::parse("COLORI\n  Default ..... #00aa11\n  Nero brand .. #101010\nTITOLI (display)\n  Poppins — weight 700\nTESTO (body)\n  Lato — default\n");
        assert_eq!((b.yellow.as_str(), b.black.as_str()), ("#00AA11", "#101010"));
        assert_eq!((b.title_font.as_str(), b.body_font.as_str()), ("Poppins", "Lato"));
        assert_eq!(b.yellow_light, "#FFE27A", "ciò che manca resta com'era");
        let broken = Brand::parse("niente di utile qui\n  Default ..... non un colore");
        assert_eq!(broken.yellow, "#FFC501");
        assert_eq!(broken.title_font, "League Spartan");
    }

    #[test]
    fn the_css_embeds_the_fonts_defines_the_tokens_and_marks_titles_in_the_brand_weight() {
        let css = Brand::default().css(true);
        assert!(css.contains("@font-face{font-family:'League Spartan'") && css.contains("@font-face{font-family:'Inter'"));
        assert!(css.matches("base64,").count() == 4 && !css.contains("http"), "font incorporati, niente di esterno");
        assert!(css.contains("--tb-yellow: #FFC501") && css.contains("--tb-black: #0B0B0C") && css.contains("--tb-offwhite: #FBFAF9"));
        assert!(css.contains("--tb-font-title: 'League Spartan', 'Inter', system-ui, sans-serif"));
        assert!(css.contains("--tb-font-body: 'Inter', system-ui, sans-serif"));
        assert!(css.contains("font-weight: 800 !important"));
        assert!(css.contains("@page :first"), "il frontespizio a pagina intera");
        assert!(!Brand::default().css(false).contains("@page :first"), "senza frontespizio la prima pagina ha i margini normali");
    }
}
