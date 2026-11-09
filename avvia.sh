#!/bin/sh
# Avvia Raccolta requisiti.
#   ./avvia.sh             menu dei colloqui (o un colloquio nuovo, se non ce ne sono)
#   ./avvia.sh --riprendi  riapre l'ultimo colloquio salvato
#   ./avvia.sh --installa  aggiunge l'app al menu delle applicazioni, per avviarla senza terminale
# Modelli, chiave Gemini e microfono si scelgono dal pannello «Impostazioni» dell'app
# (salvati in ~/.config/raccolta-requisiti/); GEMINI_API_KEY vale se lì non c'è una chiave.
cd "$(dirname "$0")" || exit 1
dir=$(pwd)
bin=./target/release/raccolta-requisiti
# Dal menu delle app il PATH può non avere cargo installato con rustup.
PATH="$HOME/.cargo/bin:$PATH"

# Un messaggio a terminale e, se si è partiti dal menu, anche come notifica.
avviso() {
    echo "$1"
    command -v notify-send >/dev/null 2>&1 && notify-send "Raccolta requisiti" "$1"
}

if [ "$1" = "--installa" ]; then
    apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    mkdir -p "$apps" || exit 1
    cat > "$apps/raccolta-requisiti.desktop" <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=Raccolta requisiti
Comment=Colloqui con i clienti: registra, trascrive e disegna il processo
Exec="$dir/avvia.sh"
Icon=audio-input-microphone
Terminal=false
Categories=Office;
Actions=riprendi;

[Desktop Action riprendi]
Name=Riprendi l'ultimo colloquio
Exec="$dir/avvia.sh" --riprendi
EOF
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" 2>/dev/null
    echo "Aggiunta al menu delle applicazioni: cerca «Raccolta requisiti»."
    exit 0
fi

# Una sola finestra alla volta: due istanze sullo stesso colloquio si sovrascriverebbero.
if pgrep -x raccolta-requis >/dev/null 2>&1; then  # il nome del processo, troncato a 15 caratteri
    avviso "Raccolta requisiti è già aperta."
    exit 0
fi

# Compila la prima volta, e di nuovo quando il codice è più recente del programma (dopo un git pull).
if [ ! -x "$bin" ] || [ -n "$(find src Cargo.toml Cargo.lock vendor -newer "$bin" -type f -print -quit 2>/dev/null)" ]; then
    avviso "Preparo la versione aggiornata: può volerci qualche minuto."
    cargo build --release || { avviso "Compilazione non riuscita: lancia ./avvia.sh da un terminale per vedere l'errore."; exit 1; }
fi

[ "$1" = "--riprendi" ] && export RR_RESUME=1
exec "$bin"
