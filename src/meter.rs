//! Il livello del microfono dal vivo, per il pannello delle impostazioni: un `ffmpeg` legge la
//! sorgente scelta e un thread porta il volume del parlato a una barra, attraverso valori dal vivo
//! (`live_rect`, `live_fill`): la barra si muove senza ricostruire la vista.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use rune::{Color, Live, oklch};

/// 400 campioni a 16 kHz: un aggiornamento ogni 25 ms.
const BLOCK: usize = 800;
/// I primi blocchi dopo l'apertura sono un picco spurio del dispositivo: si scartano.
const SKIP_BLOCKS: usize = 4;
/// Sotto questo livello (dBFS) la barra è vuota.
const FLOOR_DB: f32 = -60.0;
/// Quanto scende la barra al secondo quando il suono cala (1 = tutta la barra).
const RELEASE: f32 = 1.6;

/// Il livello di un blocco di campioni, da 0 (silenzio, sotto -60 dBFS) a 1 (fondo scala).
pub fn level_of(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum / samples.len() as f64).sqrt() / 32768.0;
    if rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.log10() as f32;
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// La barra sale subito e scende piano, come un VU-meter.
pub fn smooth(prev: f32, now: f32, dt: f32) -> f32 {
    if now >= prev { now } else { (prev - RELEASE * dt).max(now) }
}

/// Verde finché il parlato è pulito, giallo vicino al massimo, rosso quando satura.
pub fn zone_color(level: f32) -> Color {
    if level >= 0.92 {
        oklch(0.66, 0.23, 27.0)
    } else if level >= 0.78 {
        oklch(0.84, 0.17, 92.0)
    } else {
        oklch(0.74, 0.15, 150.0)
    }
}

/// Un ascolto del microfono in corso; finisce quando si lascia cadere.
pub struct LevelMeter {
    child: Child,
}

impl LevelMeter {
    /// Ascolta `source` (un nome PipeWire, o `default`) e disegna il livello in `rect` (una barra
    /// larga al massimo `width` e alta `height`) con il colore in `color`.
    pub fn start(ffmpeg: &str, source: &str, width: f32, height: f32, rect: Live<[f32; 4]>, color: Live<Color>) -> std::io::Result<LevelMeter> {
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-f", "pulse", "-fragment_size", "640", "-i", source])
            .args(["-ac", "1", "-ar", "16000", "-f", "s16le", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Se l'app muore di colpo, l'ascolto non resta acceso.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let mut out = child.stdout.take().ok_or_else(|| std::io::Error::other("nessun audio da ffmpeg"))?;
        std::thread::spawn(move || {
            let mut buf = [0u8; BLOCK];
            let (mut shown, mut blocks) = (0.0f32, 0usize);
            let mut last = std::time::Instant::now();
            while out.read_exact(&mut buf).is_ok() {
                blocks += 1;
                if blocks <= SKIP_BLOCKS {
                    continue;
                }
                let samples: Vec<i16> = buf.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                let dt = last.elapsed().as_secs_f32();
                last = std::time::Instant::now();
                shown = smooth(shown, level_of(&samples), dt);
                rect.set([0.0, 0.0, shown * width, height]);
                color.set(zone_color(shown));
            }
            rect.set([0.0, 0.0, 0.0, height]);
        });
        Ok(LevelMeter { child })
    }
}

impl Drop for LevelMeter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn silence_is_empty_and_full_scale_is_full() {
        assert_eq!(level_of(&[0; 400]), 0.0);
        assert_eq!(level_of(&[]), 0.0);
        assert!((level_of(&[i16::MAX; 400]) - 1.0).abs() < 1e-3);
        let quiet: Vec<i16> = (0..400).map(|i| if i % 2 == 0 { 33 } else { -33 }).collect();
        assert!(level_of(&quiet) < 0.01, "-60 dBFS è il fondo della barra");
        let speech: Vec<i16> = (0..400).map(|i| if i % 2 == 0 { 3277 } else { -3277 }).collect();
        assert!((level_of(&speech) - 2.0 / 3.0).abs() < 0.01, "-20 dBFS sta a due terzi");
    }

    #[test]
    fn the_bar_jumps_up_and_falls_slowly() {
        assert_eq!(smooth(0.2, 0.9, 0.025), 0.9);
        let after = smooth(0.9, 0.0, 0.025);
        assert!((after - (0.9 - RELEASE * 0.025)).abs() < 1e-6);
        assert_eq!(smooth(0.1, 0.0, 1.0), 0.0, "non scende sotto il livello vero");
    }

    #[test]
    fn colours_warn_before_clipping() {
        assert_eq!(zone_color(0.5), zone_color(0.7));
        assert_ne!(zone_color(0.5), zone_color(0.85));
        assert_ne!(zone_color(0.85), zone_color(0.95));
    }

    #[test]
    fn a_loud_source_fills_the_bar_and_stopping_kills_the_listener() {
        let dir = std::env::temp_dir().join(format!("rr-meter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Un finto ffmpeg che scrive un'onda quadra a fondo scala e poi resta in ascolto.
        let bin = dir.join("fake-ffmpeg.sh");
        std::fs::write(&bin, "#!/bin/sh\nexec python3 -I -c \"import sys,time\nb=b'\\\\xff\\\\x7f\\\\x01\\\\x80'*200\nwhile True:\n sys.stdout.buffer.write(b); sys.stdout.flush(); time.sleep(0.01)\"\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (rect, color) = (Live::new([0.0f32; 4]), Live::new(Color::BLACK));
        let meter = LevelMeter::start(bin.to_str().unwrap(), "default", 200.0, 8.0, rect.clone(), color.clone()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while rect.get()[2] < 190.0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(rect.get()[2] >= 190.0, "barra piena: {:?}", rect.get());
        assert_eq!(rect.get()[3], 8.0);
        assert_eq!(color.get(), zone_color(1.0), "rossa: satura");
        let pid = meter.child.id();
        drop(meter);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists() || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z ")), "ffmpeg fermato");
        let _ = std::fs::remove_dir_all(dir);
    }
}
