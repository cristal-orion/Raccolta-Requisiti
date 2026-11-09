# Raccolta Requisiti

App desktop per Linux che affianca il consulente durante i colloqui di raccolta requisiti con i
clienti (CRM, automazioni, siti web). Mentre il cliente racconta:

- **registra** il colloquio dal microfono scelto e lo **trascrive** separando le voci;
- un **agente** riassume quello che ha capito e disegna il processo come diagramma **Mermaid** su
  una lavagna;
- quando un passo del processo non è chiaro, l'agente propone una **domanda** accanto al nodo a
  cui si riferisce (poche, solo quelle che contano);
- note, foto e documenti trascinati nell'app finiscono anch'essi all'agente;
- alla fine esporta un **verbale in PDF** con analisi, requisiti, domande e risposte, diagrammi e
  trascrizione completa.

Interfaccia nativa in Rust con [Rune](https://github.com/anomalyco/rune); trascrizione con Google
Gemini; agente tramite la CLI di OpenAI Codex.

## Cosa c'è

- **Colloqui per cliente.** All'avvio un menu elenca i colloqui salvati, raggruppati per cliente,
  con ricerca. Da ogni colloquio si torna al menu con «Colloqui».
- **Aree di lavoro.** Un colloquio può avere più aree (una per argomento o interlocutore), ognuna
  con il suo diagramma, le sue note, i suoi allegati e il suo audio.
- **Registrazione e trascrizione dal vivo.** L'audio va in pezzi da 20 secondi, raccolti in
  finestre da 10 minuti; ogni 30 secondi la finestra in corso si rimanda intera a Gemini, così le
  voci restano coerenti al suo interno. La prima
  voce che parla è «Io»; un pulsante scambia le voci se Gemini le ha invertite.
- **Lavagna infinita.** Due dita sul touchpad spostano la lavagna (con inerzia), il pizzico e
  Ctrl + rotella ingrandiscono attorno al puntatore, si può trascinare lo sfondo o un nodo, il
  doppio clic adatta il diagramma. Un clic su un nodo apre un campo per chiedere all'agente una
  modifica al diagramma.
- **Domande.** Le domande dell'agente compaiono accanto al nodo; chiuse diventano un puntino
  giallo. Si risponde dalla lavagna o dalla scheda «Domande»: la risposta va all'agente e la
  domanda sparisce dalla lavagna.
- **Impostazioni.** Modello, ragionamento e modalità veloce dell'agente; modello, lingua e chiave
  di Gemini (con una prova della chiave); scelta del microfono, anche esterno, con il volume in
  ingresso (fino al 150%, per dare più guadagno a un microfono debole) e il livello dal vivo per
  provarlo mentre si parla.
- **Verbale PDF.** Il modello scrive struttura e analisi; il materiale originale (trascrizione,
  domande, note, allegati, diagrammi) lo inserisce il programma parola per parola. Chromium
  stampa l'HTML in PDF.

## Requisiti

- Linux, pensata per Wayland (provata su Hyprland), con PipeWire o PulseAudio (`pactl`).
- Rust stabile recente (edition 2024; sviluppato con rustc 1.99).
- `ffmpeg` con `libopus`, per registrare e unire l'audio.
- `curl`, per le chiamate a Gemini.
- [Codex CLI](https://github.com/openai/codex) con l'accesso già fatto (`codex login`): l'agente
  usa il tuo account ChatGPT o una chiave OpenAI configurata in Codex.
- Una chiave API di Google Gemini (Google AI Studio o Google Cloud), per la trascrizione.
- `chromium`, per esportare il PDF.
- Facoltativi: `pdftotext` (poppler) per far leggere i PDF allegati all'agente;
  `xdg-desktop-portal` con un FileChooser per il pulsante «Scegli file…» (in alternativa si
  trascinano i file nella finestra).

## Avvio

```sh
cargo build --release      # la prima volta ci vogliono alcuni minuti
./avvia.sh                 # menu dei colloqui (o un colloquio nuovo se non ce ne sono)
./avvia.sh --riprendi      # riapre l'ultimo colloquio salvato
```

`RR_SESSION=/percorso/della/sessione ./target/release/raccolta-requisiti` apre un colloquio
preciso.

Al primo avvio apri **Impostazioni**, incolla la chiave di Gemini e premi «Prova». Senza chiave
l'audio si registra comunque e viene trascritto appena la chiave c'è.

## Dove stanno i dati

| Cosa | Dove |
|---|---|
| Colloqui (testo, diagrammi, domande, audio, allegati, PDF) | `~/.local/share/raccolta-requisiti/sessioni/<colloquio>/` |
| Impostazioni | `~/.config/raccolta-requisiti/impostazioni.json` |
| Chiave di Gemini | `~/.config/raccolta-requisiti/gemini.key` (permessi 600) |

La chiave salvata dalle impostazioni ha la precedenza sulla variabile `GEMINI_API_KEY`. Passa a
`curl` dalla sua configurazione su stdin, mai tra gli argomenti del comando. Nessuna chiave e
nessun colloquio stanno nel repository.

**Cosa esce dal computer:** l'audio va a Google (Gemini) per la trascrizione; trascrizione, note,
allegati e diagramma vanno a OpenAI tramite Codex per l'agente e per il verbale. Valuta con il
cliente se registrare e se usare servizi cloud.

## Variabili d'ambiente

| Variabile | A cosa serve |
|---|---|
| `GEMINI_API_KEY` | Chiave di Gemini, se non è salvata nelle impostazioni |
| `RR_SESSION`, `RR_RESUME` | Apre un colloquio preciso / l'ultimo |
| `RR_CONFIG_DIR` | Cartella delle impostazioni (utile nei test) |
| `XDG_DATA_HOME`, `XDG_CONFIG_HOME` | Radici dei dati e delle impostazioni |
| `CODEX_BIN`, `CODEX_HOME` | Il comando `codex` e la sua cartella |
| `RR_FFMPEG`, `RR_CURL`, `RR_CHROMIUM` | Programmi esterni alternativi |
| `RR_LANG`, `RR_AUDIO_SOURCE` | Lingua e microfono predefiniti, se non scelti nelle impostazioni |
| `RR_NO_OPEN` | Non apre il PDF appena esportato |

## Sviluppo

```sh
cargo test --release                                    # test senza rete
cargo test --release live_agent -- --ignored --nocapture # chiama davvero Codex
RR_TEST_AUDIO=dialogo.ogg cargo test --release live_gemini -- --ignored --nocapture
```

Rune permette di disegnare l'app senza finestra, utile per controllare l'interfaccia:

```sh
./target/release/raccolta-requisiti --scene demo --snap demo.png --frames 30
./target/release/raccolta-requisiti --scene basso --script gesti.json --snap dopo.png --tree albero.json
```

Le scene (`demo`, `domande`, `basso`, `lontano`, `colloqui`, `impostazioni`, …) sono in
`src/main.rs`. Per non toccare i colloqui veri usa `XDG_DATA_HOME` e `RR_CONFIG_DIR` su una
cartella temporanea.

### Struttura

| File | Cosa fa |
|---|---|
| `src/main.rs` | Avvio, scene di anteprima, tasti |
| `src/state.rs`, `src/state/` | Stato e azioni: aree, registrazione, trascrizione, camera della lavagna, menu, impostazioni |
| `src/ui/` | Interfaccia: barra, lavagna, pannelli laterali, menu, impostazioni |
| `src/agent.rs` | Prompt e chiamata a `codex exec` con risposta JSON (riassunto, Mermaid, domande) |
| `src/transcribe.rs` | Chiamate a Gemini (trascrizione con separazione delle voci, prova della chiave) |
| `src/audio.rs` | Registrazione con `ffmpeg` dalla sorgente PipeWire scelta |
| `src/export.rs`, `src/brand.rs` | Verbale HTML/PDF con i colori del brand |
| `src/library.rs` | Elenco dei colloqui salvati per il menu |
| `src/settings.rs` | Impostazioni, chiave, microfoni e loro volume, modelli di Codex |
| `src/meter.rs` | Livello del microfono dal vivo per il pannello delle impostazioni |
| `src/model.rs` | Dati del colloquio e salvataggio |

## Componenti di terze parti

- [Rune](https://github.com/anomalyco/rune), licenza MIT, in `vendor/rune` con una modifica per il
  pizzico sul touchpad sotto Wayland: vedi `vendor/rune/MODIFICHE.md`.
- Font [Inter](https://rsms.me/inter/) e [League Spartan](https://github.com/theleagueof/league-spartan),
  licenza SIL Open Font License, in `assets/fonts` con le rispettive licenze.
