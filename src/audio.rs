//! La registrazione: un processo `ffmpeg` che legge il microfono scelto (di norma quello
//! predefinito di PipeWire) e
//! scrive file Opus da `CHUNK_SECS` secondi, ognuno chiuso e valido appena è finito (così si può
//! trascrivere mentre si parla). Fermarla è chiudere il processo con garbo; riprendere è
//! avviarne uno nuovo.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::model::CHUNK_SECS;

pub struct Recorder {
    child: Child,
    started: Instant,
}

/// Il binario di ffmpeg: `RR_FFMPEG` lo sostituisce (nei test, con uno finto).
pub fn default_bin() -> String {
    std::env::var("RR_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

impl Recorder {
    /// Avvia la registrazione da `source` (un nome PipeWire, o `default`): scrive
    /// `dir/<base>_000.ogg`, `_001.ogg`… (la cartella si crea); gli errori di ffmpeg vanno in `log`.
    pub fn start(bin: &str, source: &str, dir: &Path, base: &str, log: &Path) -> std::io::Result<Recorder> {
        std::fs::create_dir_all(dir)?;
        let log = OpenOptions::new().create(true).append(true).open(log)?;
        let mut cmd = Command::new(bin);
        cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "pulse", "-i", source])
            .args(["-ac", "1", "-ar", "16000", "-c:a", "libopus", "-b:a", "32k", "-application", "voip"])
            .args(["-f", "segment", "-segment_format", "ogg", "-reset_timestamps", "1", "-segment_time", &CHUNK_SECS.to_string()])
            .arg(dir.join(format!("{base}_%03d.ogg")))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log));
        // Se l'app muore di colpo ffmpeg non deve restare ad ascoltare: riceve SIGTERM e chiude il file.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        Ok(Recorder { child, started: Instant::now() })
    }

    /// Il processo gira ancora (non è morto per un microfono assente o occupato).
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Ferma e chiude il file; restituisce i secondi registrati.
    pub fn stop(mut self) -> f32 {
        self.finish();
        self.started.elapsed().as_secs_f32()
    }

    fn finish(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            if let Some(mut stdin) = self.child.stdin.take() {
                let _ = stdin.write_all(b"q\n");
                let _ = stdin.flush();
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while matches!(self.child.try_wait(), Ok(None)) {
                if Instant::now() > deadline {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Un finto ffmpeg: scrive il primo pezzo (l'ultimo argomento, con `%03d` → `000`) e aspetta il «q».
    pub fn fake_ffmpeg(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("fake-ffmpeg.sh");
        std::fs::write(&path, "#!/bin/sh\nfor a; do f=\"$a\"; done\nf=$(printf '%s' \"$f\" | sed 's/%03d/000/')\nprintf 'audio' > \"$f\"\nread line\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_recording_writes_its_first_chunk_and_stops_on_request() {
        let dir = std::env::temp_dir().join(format!("rr-audio-{}", std::process::id()));
        let bin = fake_ffmpeg(&dir);
        let audio = dir.join("audio");
        let mut r = Recorder::start(&bin, "default", &audio, "a0-1", &dir.join("audio/ffmpeg.log")).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(r.alive(), "gira finché non lo si ferma");
        assert!(audio.join("a0-1_000.ogg").exists());
        let secs = r.stop();
        assert!(secs >= 0.25, "{secs}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("rr-audio-missing-{}", std::process::id()));
        let r = Recorder::start("/non/esiste/ffmpeg", "default", &dir, "a0-1", &dir.join("l.log"));
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Registra 26 secondi dal microfono vero e controlla i pezzi:
    /// `cargo test --release live_mic -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_mic_records_chunks_that_are_closed_and_playable() {
        let dir = std::env::temp_dir().join(format!("rr-mic-{}", std::process::id()));
        let audio = dir.join("audio");
        let r = Recorder::start(&default_bin(), "default", &audio, "a0-1", &audio.join("ffmpeg.log")).unwrap();
        std::thread::sleep(Duration::from_secs(26));
        let first = audio.join("a0-1_000.ogg");
        let second = audio.join("a0-1_001.ogg");
        println!("a 26 s: primo pezzo chiuso e secondo già aperto = {}", first.exists() && second.exists());
        assert!(first.exists() && second.exists(), "dopo 20 s il primo pezzo è chiuso e ne è iniziato un altro");
        let secs = r.stop();
        for f in [&first, &second] {
            let len = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            println!("{} {len} byte", f.file_name().unwrap().to_string_lossy());
            assert!(len > 1000);
        }
        println!("{secs:.1} s totali");
        let _ = std::fs::remove_dir_all(dir);
    }
}
