#!/bin/sh
# Avvia Raccolta requisiti.
#   ./avvia.sh             sessione nuova
#   ./avvia.sh --riprendi  riapre l'ultima sessione salvata
# Modelli, chiave Gemini e microfono si scelgono dal pannello «Impostazioni» dell'app
# (salvati in ~/.config/raccolta-requisiti/); GEMINI_API_KEY vale se lì non c'è una chiave.
cd "$(dirname "$0")" || exit 1
[ "$1" = "--riprendi" ] && export RR_RESUME=1
[ -x ./target/release/raccolta-requisiti ] || cargo build --release || exit 1
exec ./target/release/raccolta-requisiti
