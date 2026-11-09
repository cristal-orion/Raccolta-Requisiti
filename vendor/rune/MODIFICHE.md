# Rune: copia locale con modifiche

Questa cartella è una copia di [Rune](https://github.com/anomalyco/rune) (licenza MIT, vedi `LICENSE`)
alla revisione [`2bdfae1`](https://github.com/anomalyco/rune/commit/2bdfae16568782a56cb93818944d05b93546bce5)
del ramo `dev`, con due differenze:

1. **Pizzico sul touchpad sotto Wayland.** winit non riporta i gesti di pizzico su Wayland: il file
   nuovo `crates/rune/src/platform/wayland_pinch.rs` si lega al protocollo `zwp_pointer_gestures_v1`
   del compositor e consegna il pizzico agli `on_magnify` come su macOS. Per questo
   `wayland-protocols` ha anche la feature `unstable`, e `lib.rs`, `platform.rs` e `window/*.rs`
   collegano il listener alla finestra (`touchpad_pinch_active()` e `touchpad_pinch_problem()`
   dicono se funziona).
2. **Workspace sfoltito.** Il `Cargo.toml` alla radice non elenca gli esempi, le demo e il sito di
   Rune, che qui non ci sono.

Tutte le modifiche al codice sono in `raccolta-requisiti.patch` (formato `diff -u`, rispetto a
`2bdfae1`).
